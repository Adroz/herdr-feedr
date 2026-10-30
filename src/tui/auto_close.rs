//! Sidebar auto-close (spec #22): exit once the sidebar is the only pane
//! left in its tab, so herdr closes the tab — and stop herdr when nothing
//! else is left anywhere.
//!
//! ```text
//! start   --(first successful pane.list shows company in our tab)--> Armed
//!    tried on the first tick, then on each layout until one succeeds
//! Unarmed --(layout for our tab shows company)--> Armed
//! Armed   --(layout for our tab shows only feedr panes)--> debounce
//! debounce expires --> pane.list re-check, filtered to our live tab
//!    company present / query failed --> stay Armed
//!    alone, a non-feedr pane elsewhere --> exit
//!    alone, nothing else anywhere      --> server.stop (if enabled), exit
//! layout showing company during the debounce --> cancel it
//! (re)subscribed --> pane.list, then judged as a layout of our live tab
//!    (a replaced stream's unread layout.updated is lost; query failed:
//!    no decision)
//! layout with a pane pane.list can't identify (query failed)
//!    --> no decision: never arms; an armed debounce is cancelled
//! ```
//!
//! Company is any pane that isn't a feedr sidebar: other plugins' panes,
//! overlays and zoomed panes all count. Arming only once company is seen
//! means a sidebar opened alone never closes itself straight away. The
//! layout that opening the sidebar produces fires before we subscribe, so a
//! startup `pane.list` is what sees the shell we were opened beside.

use crate::tui::socket::{own_tab, Herdr, PaneEntry};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long the tab must stay companion-less before the re-check, so layout
/// churn (a pane replaced, a split rearranged) doesn't close the sidebar.
pub const DEBOUNCE: Duration = Duration::from_millis(400);

/// What the app should do after a tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Stay,
    Exit,
    /// Nothing else is left in herdr: stop the server, then exit.
    StopServerThenExit,
}

#[derive(Default)]
pub struct AutoClose {
    armed: bool,
    /// A startup `pane.list` has succeeded. Until then the first tick and
    /// every layout try one; a failure never arms.
    listed: bool,
    /// The first tick has run (and made its startup `pane.list` attempt).
    ticked: bool,
    /// Set while armed and a layout showed us alone; cleared by company.
    alone_since: Option<Instant>,
    /// pane id -> is a feedr sidebar, from the last successful `pane.list`.
    /// Layout events carry only ids, so titles come from here.
    feedr: HashMap<String, bool>,
}

impl AutoClose {
    /// A `layout.updated` arrived. Ignored unless it is our tab's.
    pub fn on_layout(
        &mut self,
        own: &str,
        pane_ids: &[String],
        now: Instant,
        herdr: &mut dyn Herdr,
    ) {
        self.list_at_start(own, herdr);
        if !pane_ids.iter().any(|p| p == own) {
            return;
        }
        let others: Vec<&String> = pane_ids.iter().filter(|p| *p != own).collect();
        if others.iter().any(|p| !self.feedr.contains_key(*p)) {
            if let Ok(panes) = herdr.list_panes() {
                self.remember(&panes);
            }
        }
        // An id still unknown means the query failed: no decision. It
        // mustn't arm (arming needs company actually seen), and an armed
        // sidebar drops any debounce — the safe direction is to stay.
        if others.iter().any(|p| !self.feedr.contains_key(*p)) {
            self.alone_since = None;
            return;
        }
        let company = others
            .iter()
            .any(|p| !self.feedr.get(*p).copied().unwrap_or(false));
        if company {
            self.armed = true;
            self.alone_since = None;
        } else if self.armed && self.alone_since.is_none() {
            self.alone_since = Some(now);
        }
    }

    /// The event stream was (re)subscribed. herdr drops the stream the
    /// subscriber replaces, unread `layout.updated` and all, so judge our
    /// live tab from `pane.list` as if its layout had just arrived.
    pub fn on_subscribed(&mut self, own: &str, now: Instant, herdr: &mut dyn Herdr) {
        let Ok(panes) = herdr.list_panes() else {
            return; // no decision
        };
        self.listed = true;
        self.remember(&panes);
        let Some(tab) = own_tab(&panes, own) else {
            return;
        };
        let pane_ids: Vec<String> = panes
            .iter()
            .filter(|p| p.tab_id == tab)
            .map(|p| p.pane_id.clone())
            .collect();
        self.on_layout(own, &pane_ids, now, herdr);
    }

    /// Run the re-check once the debounce has elapsed.
    pub fn tick(
        &mut self,
        own: &str,
        now: Instant,
        quit_when_last: bool,
        herdr: &mut dyn Herdr,
    ) -> Verdict {
        if !self.ticked {
            self.ticked = true;
            self.list_at_start(own, herdr);
        }
        match self.alone_since {
            Some(t) if now.duration_since(t) >= DEBOUNCE => {}
            _ => return Verdict::Stay,
        }
        self.alone_since = None; // stay armed whatever the outcome
        let Ok(panes) = herdr.list_panes() else {
            return Verdict::Stay; // never close on error
        };
        self.remember(&panes);
        let Some(tab) = own_tab(&panes, own) else {
            return Verdict::Stay;
        };
        if panes.iter().any(|p| p.tab_id == tab && is_company(p, own)) {
            return Verdict::Stay;
        }
        if panes.iter().any(|p| is_company(p, own)) || !quit_when_last {
            Verdict::Exit
        } else {
            Verdict::StopServerThenExit
        }
    }

    /// Until a `pane.list` has succeeded, try one and arm if it shows
    /// company in our tab. A failed query doesn't arm.
    fn list_at_start(&mut self, own: &str, herdr: &mut dyn Herdr) {
        if self.listed {
            return;
        }
        let Ok(panes) = herdr.list_panes() else {
            return;
        };
        self.listed = true;
        self.remember(&panes);
        if let Some(tab) = own_tab(&panes, own) {
            self.armed |= panes.iter().any(|p| p.tab_id == tab && is_company(p, own));
        }
    }

    fn remember(&mut self, panes: &[PaneEntry]) {
        self.feedr = panes
            .iter()
            .map(|p| (p.pane_id.clone(), p.is_feedr()))
            .collect();
    }
}

/// Any pane but ourselves and other feedr sidebars.
fn is_company(p: &PaneEntry, own: &str) -> bool {
    p.pane_id != own && !p.is_feedr()
}

#[cfg(test)]
mod tests {
    use crate::tui::driver::{Driver, PANE_ID, TAB_ID};
    use std::time::Duration;

    const FEED: &str = "# Feed\n\n- [ ] Alpha\n";
    const SIDEBAR: &str = "feedr-sidebar";

    fn sidebar() -> Driver {
        Driver::new(FEED, 40, 12)
    }

    #[test]
    fn armed_sidebar_left_alone_exits_after_the_debounce() {
        let mut d = sidebar();
        d.set_panes(&[
            (PANE_ID, TAB_ID, SIDEBAR),
            ("w1:p2", TAB_ID, "zsh"),
            ("w1:p3", "w1:t2", "zsh"),
        ]);
        d.layout(TAB_ID, &[PANE_ID, "w1:p2"]); // company seen: armed
        d.set_panes(&[(PANE_ID, TAB_ID, SIDEBAR), ("w1:p3", "w1:t2", "zsh")]);
        d.layout(TAB_ID, &[PANE_ID]); // the shell closed
        d.advance(Duration::from_millis(100));
        assert!(!d.exit_requested(), "must wait out the debounce");
        d.advance(Duration::from_millis(400));
        assert!(d.exit_requested());
        assert!(
            !d.herdr_calls().contains(&"stop_server".to_string()),
            "a pane remains elsewhere, so herdr keeps running"
        );
    }

    const SHELL: &str = "w1:p2";
    const ELSEWHERE: (&str, &str, &str) = ("w1:p3", "w1:t2", "zsh");

    fn stopped(d: &Driver) -> bool {
        d.herdr_calls().contains(&"stop_server".to_string())
    }

    /// Armed by a shell beside us, then the shell goes: herdr's layout says
    /// we're alone and `pane.list` will agree (unless the test changes it).
    fn armed_then_left_alone(d: &mut Driver) {
        d.set_panes(&[
            (PANE_ID, TAB_ID, SIDEBAR),
            (SHELL, TAB_ID, "zsh"),
            ELSEWHERE,
        ]);
        d.layout(TAB_ID, &[PANE_ID, SHELL]);
        d.set_panes(&[(PANE_ID, TAB_ID, SIDEBAR), ELSEWHERE]);
        d.layout(TAB_ID, &[PANE_ID]);
    }

    #[test]
    fn a_sidebar_that_never_had_company_never_closes() {
        let mut d = sidebar();
        d.set_panes(&[(PANE_ID, TAB_ID, SIDEBAR), ELSEWHERE]);
        d.layout(TAB_ID, &[PANE_ID]);
        d.advance(Duration::from_secs(2));
        assert!(!d.exit_requested());
    }

    #[test]
    fn a_sidebar_opened_beside_a_shell_arms_without_seeing_a_layout() {
        // Opening the sidebar fires its layout.updated before we subscribe,
        // so the first thing we hear is the shell closing (#46).
        let mut d = Driver::starting_with(
            FEED,
            40,
            12,
            &[
                (PANE_ID, TAB_ID, SIDEBAR),
                (SHELL, TAB_ID, "zsh"),
                ELSEWHERE,
            ],
        );
        d.set_panes(&[(PANE_ID, TAB_ID, SIDEBAR), ELSEWHERE]);
        d.layout(TAB_ID, &[PANE_ID]);
        d.advance(Duration::from_millis(100));
        assert!(!d.exit_requested(), "must wait out the debounce");
        d.advance(Duration::from_millis(400));
        assert!(d.exit_requested());
    }

    #[test]
    fn a_sidebar_started_alone_in_its_tab_does_not_arm() {
        let mut d = Driver::starting_with(FEED, 40, 12, &[(PANE_ID, TAB_ID, SIDEBAR), ELSEWHERE]);
        d.layout(TAB_ID, &[PANE_ID]);
        d.advance(Duration::from_secs(2));
        assert!(!d.exit_requested());
    }

    #[test]
    fn a_failed_initial_pane_list_does_not_arm() {
        let mut d = sidebar(); // pane.list fails until set_panes
        d.set_panes(&[(PANE_ID, TAB_ID, SIDEBAR), ELSEWHERE]);
        d.layout(TAB_ID, &[PANE_ID]);
        d.advance(Duration::from_secs(2));
        assert!(!d.exit_requested());
    }

    #[test]
    fn a_failed_startup_list_is_retried_on_the_next_layout() {
        let mut d = sidebar(); // the startup pane.list fails
        d.set_panes(&[
            (PANE_ID, TAB_ID, SIDEBAR),
            (SHELL, TAB_ID, "zsh"),
            ELSEWHERE,
        ]);
        d.layout("w1:t2", &[ELSEWHERE.0]); // any layout: try again, see the shell
        d.set_panes(&[(PANE_ID, TAB_ID, SIDEBAR), ELSEWHERE]);
        d.layout(TAB_ID, &[PANE_ID]);
        d.advance(Duration::from_secs(1));
        assert!(d.exit_requested());
    }

    #[test]
    fn a_layout_lost_to_a_resubscribe_is_recovered_from_pane_list() {
        let mut d = sidebar();
        d.set_panes(&[
            (PANE_ID, TAB_ID, SIDEBAR),
            (SHELL, TAB_ID, "zsh"),
            ELSEWHERE,
        ]);
        d.layout(TAB_ID, &[PANE_ID, SHELL]); // armed
                                             // The agent shell exits. Its layout.updated sat unread on the
                                             // stream the subscriber replaced, so only the resubscribe arrives.
        d.set_panes(&[(PANE_ID, TAB_ID, SIDEBAR), ELSEWHERE]);
        d.subscribed();
        d.advance(Duration::from_millis(100));
        assert!(!d.exit_requested(), "must wait out the debounce");
        d.advance(Duration::from_millis(400));
        assert!(d.exit_requested());
    }

    #[test]
    fn a_resubscribe_never_arms_a_sidebar_that_is_alone() {
        let mut d = Driver::starting_with(FEED, 40, 12, &[(PANE_ID, TAB_ID, SIDEBAR), ELSEWHERE]);
        d.subscribed();
        d.advance(Duration::from_secs(2));
        assert!(!d.exit_requested());
    }

    #[test]
    fn a_resubscribe_whose_pane_list_fails_keeps_the_sidebar() {
        let mut d = sidebar();
        d.set_panes(&[(PANE_ID, TAB_ID, SIDEBAR), (SHELL, TAB_ID, "zsh")]);
        d.layout(TAB_ID, &[PANE_ID, SHELL]); // armed
        d.fail_pane_list();
        d.subscribed();
        d.advance(Duration::from_secs(2));
        assert!(!d.exit_requested());
    }

    #[test]
    fn company_returning_during_the_debounce_cancels_it() {
        let mut d = sidebar();
        armed_then_left_alone(&mut d);
        d.advance(Duration::from_millis(200));
        d.set_panes(&[
            (PANE_ID, TAB_ID, SIDEBAR),
            ("w1:p4", TAB_ID, "zsh"),
            ELSEWHERE,
        ]);
        d.layout(TAB_ID, &[PANE_ID, "w1:p4"]); // a new shell split in
        d.advance(Duration::from_secs(2));
        assert!(!d.exit_requested());
    }

    #[test]
    fn churn_restarts_the_debounce_from_the_latest_lone_layout() {
        let mut d = sidebar();
        armed_then_left_alone(&mut d); // alone at t=0
        d.advance(Duration::from_millis(200));
        d.layout(TAB_ID, &[PANE_ID, SHELL]); // the shell is back...
        d.advance(Duration::from_millis(100));
        d.layout(TAB_ID, &[PANE_ID]); // ...and gone again at t=300
        d.advance(Duration::from_millis(200)); // t=500: 400ms since t=0
        assert!(!d.exit_requested(), "debounce counts from t=300, not t=0");
        d.advance(Duration::from_millis(200)); // t=700
        assert!(d.exit_requested());
    }

    #[test]
    fn a_re_check_that_still_finds_company_keeps_the_sidebar() {
        let mut d = sidebar();
        armed_then_left_alone(&mut d);
        // herdr's live answer disagrees with the layout event: stay.
        d.set_panes(&[(PANE_ID, TAB_ID, SIDEBAR), ("w1:p4", TAB_ID, "zsh")]);
        d.advance(Duration::from_secs(1));
        assert!(!d.exit_requested());
    }

    #[test]
    fn a_failed_re_check_never_closes_and_stays_armed() {
        let mut d = sidebar();
        armed_then_left_alone(&mut d);
        d.fail_pane_list();
        d.advance(Duration::from_secs(1));
        assert!(!d.exit_requested(), "a query error must not read as alone");

        // Still armed: the next lone layout, confirmed, closes it.
        d.set_panes(&[(PANE_ID, TAB_ID, SIDEBAR), ELSEWHERE]);
        d.layout(TAB_ID, &[PANE_ID]);
        d.advance(Duration::from_secs(1));
        assert!(d.exit_requested());
    }

    #[test]
    fn a_failed_lookup_of_an_unknown_pane_does_not_arm() {
        let mut d = sidebar();
        // A pane we can't identify joins while pane.list is failing...
        d.fail_pane_list();
        d.layout(TAB_ID, &[PANE_ID, "w1:p5"]);
        // ...and turns out to be another feedr sidebar, which then goes.
        d.set_panes(&[
            (PANE_ID, TAB_ID, SIDEBAR),
            ("w1:p5", TAB_ID, SIDEBAR),
            ELSEWHERE,
        ]);
        d.layout(TAB_ID, &[PANE_ID, "w1:p5"]);
        d.set_panes(&[(PANE_ID, TAB_ID, SIDEBAR), ELSEWHERE]);
        d.layout(TAB_ID, &[PANE_ID]);
        d.advance(Duration::from_secs(2));
        assert!(!d.exit_requested(), "it never saw company, so never armed");
    }

    #[test]
    fn a_failed_lookup_keeps_an_armed_sidebar_open() {
        let mut d = sidebar();
        armed_then_left_alone(&mut d);
        // An unknown pane joins while pane.list fails: no decision, stay.
        d.fail_pane_list();
        d.layout(TAB_ID, &[PANE_ID, "w1:p5"]);
        d.advance(Duration::from_secs(2));
        assert!(!d.exit_requested());
    }

    #[test]
    fn the_last_pane_in_herdr_stops_the_server_then_exits() {
        let mut d = sidebar();
        d.set_panes(&[(PANE_ID, TAB_ID, SIDEBAR), (SHELL, TAB_ID, "zsh")]);
        d.layout(TAB_ID, &[PANE_ID, SHELL]);
        d.set_panes(&[(PANE_ID, TAB_ID, SIDEBAR)]);
        d.layout(TAB_ID, &[PANE_ID]);
        d.advance(Duration::from_secs(1));
        assert!(d.exit_requested());
        assert_eq!(
            d.herdr_calls().last().map(String::as_str),
            Some("stop_server")
        );
    }

    #[test]
    fn quit_herdr_when_last_off_exits_without_stopping_herdr() {
        let mut d = Driver::with_config(FEED, 40, 12, |c| c.quit_herdr_when_last = false);
        d.set_panes(&[(PANE_ID, TAB_ID, SIDEBAR), (SHELL, TAB_ID, "zsh")]);
        d.layout(TAB_ID, &[PANE_ID, SHELL]);
        d.set_panes(&[(PANE_ID, TAB_ID, SIDEBAR)]);
        d.layout(TAB_ID, &[PANE_ID]);
        d.advance(Duration::from_secs(1));
        assert!(d.exit_requested());
        assert!(!stopped(&d));
    }

    #[test]
    fn auto_close_off_does_nothing_at_all() {
        let mut d = Driver::with_config(FEED, 40, 12, |c| c.auto_close = false);
        armed_then_left_alone(&mut d);
        d.advance(Duration::from_secs(1));
        assert!(!d.exit_requested());
        assert!(d.herdr_calls().is_empty(), "got: {:?}", d.herdr_calls());
    }

    #[test]
    fn other_feedr_sidebars_are_not_company() {
        let mut d = sidebar();
        let twin = ("w1:p5", TAB_ID, SIDEBAR);
        d.set_panes(&[
            (PANE_ID, TAB_ID, SIDEBAR),
            twin,
            (SHELL, TAB_ID, "zsh"),
            ELSEWHERE,
        ]);
        d.layout(TAB_ID, &[PANE_ID, "w1:p5", SHELL]);
        d.set_panes(&[(PANE_ID, TAB_ID, SIDEBAR), twin, ELSEWHERE]);
        d.layout(TAB_ID, &[PANE_ID, "w1:p5"]);
        d.advance(Duration::from_secs(1));
        assert!(d.exit_requested());
        assert!(!stopped(&d));
    }

    #[test]
    fn last_pane_check_ignores_feedr_sidebars_in_other_tabs() {
        let mut d = sidebar();
        let other_sidebar = ("w1:p6", "w1:t2", SIDEBAR);
        d.set_panes(&[
            (PANE_ID, TAB_ID, SIDEBAR),
            (SHELL, TAB_ID, "zsh"),
            other_sidebar,
        ]);
        d.layout(TAB_ID, &[PANE_ID, SHELL]);
        d.set_panes(&[(PANE_ID, TAB_ID, SIDEBAR), other_sidebar]);
        d.layout(TAB_ID, &[PANE_ID]);
        d.advance(Duration::from_secs(1));
        assert!(d.exit_requested());
        assert!(stopped(&d), "only feedr is left, so herdr quits");
    }

    #[test]
    fn other_plugins_panes_and_zoomed_overlays_are_company() {
        let mut d = sidebar();
        // A file-viewer split and a zoomed overlay: herdr lists both as
        // ordinary layout panes of our tab.
        let viewer = ("w1:p7", TAB_ID, "file-viewer");
        let overlay = ("w1:p8", TAB_ID, "lazygit");
        d.set_panes(&[
            (PANE_ID, TAB_ID, SIDEBAR),
            (SHELL, TAB_ID, "zsh"),
            viewer,
            overlay,
        ]);
        d.layout(TAB_ID, &[PANE_ID, SHELL, "w1:p7", "w1:p8"]);
        d.set_panes(&[(PANE_ID, TAB_ID, SIDEBAR), viewer, overlay]);
        d.layout(TAB_ID, &[PANE_ID, "w1:p7", "w1:p8"]);
        d.advance(Duration::from_secs(1));
        assert!(!d.exit_requested());

        d.set_panes(&[(PANE_ID, TAB_ID, SIDEBAR), overlay]);
        d.layout(TAB_ID, &[PANE_ID, "w1:p8"]);
        d.advance(Duration::from_secs(1));
        assert!(!d.exit_requested(), "the overlay alone still counts");
    }

    #[test]
    fn layouts_of_other_tabs_are_ignored() {
        let mut d = sidebar();
        armed_then_left_alone(&mut d);
        d.layout("w1:t2", &["w1:p3", "w1:p9"]);
        d.advance(Duration::from_secs(1));
        assert!(d.exit_requested(), "another tab's churn doesn't cancel");
    }

    #[test]
    fn the_re_check_follows_our_live_tab_after_a_move() {
        let mut d = sidebar();
        // We were moved to w1:t3 next to a shell; HERDR_TAB_ID is stale.
        d.set_panes(&[
            (PANE_ID, "w1:t3", SIDEBAR),
            (SHELL, "w1:t3", "zsh"),
            ELSEWHERE,
        ]);
        d.layout("w1:t3", &[PANE_ID, SHELL]);
        // Layout says alone, but the live tab still holds the shell.
        d.layout("w1:t3", &[PANE_ID]);
        d.advance(Duration::from_secs(1));
        assert!(!d.exit_requested());
    }
}
