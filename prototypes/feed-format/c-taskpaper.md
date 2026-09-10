Feed:
	- Fix auth redirect loop
		Repro: expired session → login → bounces between /login and /app forever.
		Suspect token expiry check in session middleware uses < not <=.
		See issue #142 for the original report.
	- Migrate CI to GitHub Actions @doing @agent(0198f3ab-7c2e-4b8d)
		Keep the CircleCI config until two green runs confirm parity.
		Secrets already mirrored into repo settings.
	- Write onboarding doc
	- Bump Node to 22 @done
Later:
	- Evaluate pnpm catalogs
		Blocked on the CI migration landing first.
