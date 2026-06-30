## [1.68.0](https://github.com/Meridiona/meridian/compare/v1.67.0...v1.68.0) (2026-06-30)

### 🚀 Features

* **capture:** timed pause, work hours schedule, and tracking_paused gap kind ([efbdcf1](https://github.com/Meridiona/meridian/commit/efbdcf191c4747cd494d76fcfb9dff1f4ee3ea31)), closes [#F59E0B](https://github.com/Meridiona/meridian/issues/F59E0B)
* **coding-agent:** hour-boundary sealing, 2-min poll, local-hour segmentation ([6c53481](https://github.com/Meridiona/meridian/commit/6c53481b4f51ac3eb5df35b9c31d4da604a953be))
* **dashboards:** OpenObserve dashboards for activity summary, PM worklog hour, session distiller ([1d29f2c](https://github.com/Meridiona/meridian/commit/1d29f2cf10a28f1b26761795f489029a06f6ed86))
* **etl:** filter coding-agent terminal titles from window_titles in extractor ([70768c8](https://github.com/Meridiona/meridian/commit/70768c85e223a51bf9e6610266646c9b80085d18))
* **observability:** Python ships directly to OpenObserve, no Rust daemon required ([5acff2b](https://github.com/Meridiona/meridian/commit/5acff2bfcdee8ee7cefbed30a61615d821b56042))
* **proposed-ticket-approval:** UI + API for reviewing proposed new tickets ([5bbcbd9](https://github.com/Meridiona/meridian/commit/5bbcbd9220dd15106f44bf5c75a43eba241e0eff))
* **trello:** save API key to .env before OAuth flow, prompt in UI when missing ([c7f0d9e](https://github.com/Meridiona/meridian/commit/c7f0d9e584bc31dd75912d26125ff22d834c06b7))
* **worklog-pipeline:** Python pipeline enhancements — local-hour alignment, proposed-ticket drafting, IST label fix ([62a7d74](https://github.com/Meridiona/meridian/commit/62a7d740ce583d32e7a2e10e4c4859daaf3d189a))
* **worklogs-reader:** add window_end, is_proposed, proposed_id to WorklogItem ([d41e0b6](https://github.com/Meridiona/meridian/commit/d41e0b6ce45037fe7ba3bf96391fad7d00ae8d20))

### 🐛 Bug Fixes

* **ci:** scope runtime workflow to least-privilege permissions ([cba2b0f](https://github.com/Meridiona/meridian/commit/cba2b0f3fef493cdabb155cfbdb2f9997ccaf123))
* **etl:** remove is_version_label and update tests ([ae9331a](https://github.com/Meridiona/meridian/commit/ae9331ac74d6bcf4d37d9473655783f70afe68d3))
* **linear:** group by project name when no parent, show real error in hygiene dialog ([9eb7654](https://github.com/Meridiona/meridian/commit/9eb7654e8c37f05a50e3dfe09cb37a4b55bda561))
* **pause:** address review findings on PR [#374](https://github.com/Meridiona/meridian/issues/374) ([837b656](https://github.com/Meridiona/meridian/commit/837b6561e469a20d7b0b4fdecfe4ea0f2aa988c6))
* **pause:** address three review comments on PR [#367](https://github.com/Meridiona/meridian/issues/367) ([cf5d47f](https://github.com/Meridiona/meridian/commit/cf5d47f2da59a7bd95f5a210dea49c629ebad6c1))
* **pause:** emit status-update immediately after pause/resume ([94dd03c](https://github.com/Meridiona/meridian/commit/94dd03c8301d3ff03f2904ecfccc7ad19cdb000e))
* **pause:** fix custom duration input race + Enter key; add pause tests ([4fc9401](https://github.com/Meridiona/meridian/commit/4fc94010ea221ca0538bd2f2f80ed194cf9eb480)), closes [#custom-mins](https://github.com/Meridiona/meridian/issues/custom-mins)
* **pause:** stop capture engine fully on pause, restart on resume ([01b3917](https://github.com/Meridiona/meridian/commit/01b3917ff581c81c7b261d7a8c1fd7f364fb6125))
* **pause:** use oneshot cancel channels instead of AbortHandle ([6f7b2eb](https://github.com/Meridiona/meridian/commit/6f7b2eb9fc709bde2e9f72d7155ecd201813b4d9))
* **review:** address PR [#362](https://github.com/Meridiona/meridian/issues/362) review comments ([999da78](https://github.com/Meridiona/meridian/commit/999da784c5ebb7c0fe3a3d51f493d7d2ad35051a))
* **runtime:** authenticate the python-build-standalone API lookup ([3bfe1ff](https://github.com/Meridiona/meridian/commit/3bfe1ff5c682a07f193dfebb5e041f0752f7dd54))
* **runtime:** check out repo in publish jobs so the publish script is found ([e3349d2](https://github.com/Meridiona/meridian/commit/e3349d2839775161fe08d8a40f628f5693c559fe)), closes [#372](https://github.com/Meridiona/meridian/issues/372) [#372](https://github.com/Meridiona/meridian/issues/372)
* **ticket-update:** anchor subprocess CWD to ~/.meridian so dotenvy loads .env ([5352fdb](https://github.com/Meridiona/meridian/commit/5352fdb51661ebba17857cc6f332ccde9d7c31d2))
* **tray:** restore live session block accidentally removed from index.html ([49e7454](https://github.com/Meridiona/meridian/commit/49e7454d5d7a4f1dac958d8641e18c90d6bbd50c)), closes [#app-glyph](https://github.com/Meridiona/meridian/issues/app-glyph)
* **trello:** disconnect cleanup, sentinel constant, reuse Field component ([af68681](https://github.com/Meridiona/meridian/commit/af68681fb6493679b3e21108ab171a526fa4d031))
* **worklog-pipeline:** use local-time hour labels, drop yesterday backfill ([a09b19b](https://github.com/Meridiona/meridian/commit/a09b19bd0b0570c155d44838bb166c841e49905b))
* **worklog:** restore WORKLOG_SYSTEM re-export broken by package restore ([4c1d90f](https://github.com/Meridiona/meridian/commit/4c1d90fdb196e60514a2b30f4efc19ae60d45f07))

### ♻️ Refactoring

* **indexer:** move local_hour_start_utc into meridian_core::date ([06c0916](https://github.com/Meridiona/meridian/commit/06c0916d3eed9e66f01ed8a30c2a22e73265365c))

### 🤖 CI

* **runtime:** auto-publish runtime on merge with a version gate ([60d0357](https://github.com/Meridiona/meridian/commit/60d0357c3f532b8701dd31cc56cc08848b675c46)), closes [#2](https://github.com/Meridiona/meridian/issues/2) [#3](https://github.com/Meridiona/meridian/issues/3)
* **runtime:** run gate decision-table self-test as a PR check ([9416e43](https://github.com/Meridiona/meridian/commit/9416e434d1074e68f7894cd14c0500941ecf8027))
* **runtime:** smoke-test imports every agents submodule ([274cb02](https://github.com/Meridiona/meridian/commit/274cb02881b5d4ca60f6ef6401a87b3583f598ba))

### 📝 Documentation

* **scripts:** add README for dev scripts (distill, activity report) ([0bb6cb3](https://github.com/Meridiona/meridian/commit/0bb6cb3418194d47eaab676fe33eeb8874951833))

### 🔧 Chores

* add .DS_Store to .gitignore ([d5cd09f](https://github.com/Meridiona/meridian/commit/d5cd09f7f306198fc3424ca57842a7d57b13d3ed))
* **deps:** add transformers>=5.0.0 and sentence-transformers>=3 to mlx extra ([a937ee2](https://github.com/Meridiona/meridian/commit/a937ee2ca572239c915c3df1a9dd1fd7034f50da))
* ignore ui/data/ (local OpenObserve runtime data) ([9a25b86](https://github.com/Meridiona/meridian/commit/9a25b861a941b3eac68367e45d1dd1580ac52eb2))
* **infra:** dev-start, install, build scripts, Cargo deps, skill prompt update ([67468e3](https://github.com/Meridiona/meridian/commit/67468e306af31769b701b1349ab4c738cac94396))
* **merge:** resolve conflicts with origin/pre-main ([7bbc06b](https://github.com/Meridiona/meridian/commit/7bbc06bc81496691e084b6389c3e92a02416b94d))

## [1.67.0](https://github.com/Meridiona/meridian/compare/v1.66.2...v1.67.0) (2026-06-28)

### 🚀 Features

* **azure-devops:** populate parent_key from System.Parent field ([2010d76](https://github.com/Meridiona/meridian/commit/2010d7638113aeb6ecb6b4f2ca4a89d096271ff6))
* **azure-devops:** resolve epic_title by walking parent chain ([25b9666](https://github.com/Meridiona/meridian/commit/25b9666cee8eb6b054c3df4d4ee3e163d693c128))
* **setup:** two-step provision checklist + self-check causes on error ([23db097](https://github.com/Meridiona/meridian/commit/23db0978fec3d87bb32948599d01eb85f057425d))
* **tasks:** add persistent refresh button + Jira OAuth resolve tests ([03bd591](https://github.com/Meridiona/meridian/commit/03bd591ac4f5a65fa2e8592272514beb04619037))

### 🐛 Bug Fixes

* **azure-devops:** address review findings from PR [#354](https://github.com/Meridiona/meridian/issues/354) ([a2e09d8](https://github.com/Meridiona/meridian/commit/a2e09d8acf26fdde5235ff94b48fd5f3feeabbb6))
* **azure-devops:** surface network errors and add manual org entry fallback ([4675696](https://github.com/Meridiona/meridian/commit/46756963c04f284372af393f44c8e189d8cfa356))
* **banner:** hide must-fix banner on cleanup page with trailing slash ([585c248](https://github.com/Meridiona/meridian/commit/585c248d3d60995fa7649c48b1095f482ef1fa26))
* **capture:** skip self-capture and show Meridian as process name ([c893b8f](https://github.com/Meridiona/meridian/commit/c893b8f5a7ed73fd075574eb8a0e108f833a6650))
* **cleanup:** hide disconnected-provider tasks from cleanup board and must-fix banner ([045e63c](https://github.com/Meridiona/meridian/commit/045e63ca89bb2c4b832fc06d6f2187cf37915e3b))
* **integrations:** save API tokens on a fresh install (no .env yet) ([4bfbf06](https://github.com/Meridiona/meridian/commit/4bfbf06d5a73e7d196c827d9d07c53fcb925b93e))
* **integrations:** surface real Azure DevOps error instead of generic fallback ([e495d14](https://github.com/Meridiona/meridian/commit/e495d1478710fa36206cb7b4f4fdf526e6cb9a07))
* **integrations:** use double cast to satisfy TS strict type check ([a799a5a](https://github.com/Meridiona/meridian/commit/a799a5a5f9062671a6c8611f5d4dc52e095c5b49))
* **linear/tests:** use anyhow::Result + context in DB integration tests ([1cea960](https://github.com/Meridiona/meridian/commit/1cea960c7647fda3e8041a2f711dd05fea897be4))
* **oauth:** start connect poll under StrictMode + serialize Jira refresh across processes ([eee2e22](https://github.com/Meridiona/meridian/commit/eee2e22ccba4ba97eed77b11023515c4f4a216e1))
* **prefetch:** reject non-positive env overrides instead of clamping to 1 ([ee81095](https://github.com/Meridiona/meridian/commit/ee81095d29d4b6ab4c69757d12350e52ceda49cd))
* **review:** address code-review findings from PR [#352](https://github.com/Meridiona/meridian/issues/352) ([8d5752e](https://github.com/Meridiona/meridian/commit/8d5752e3fd45b3568f18861cb763174e6db3b632))
* **review:** address CodeRabbit findings from PR [#357](https://github.com/Meridiona/meridian/issues/357) ([6f8630f](https://github.com/Meridiona/meridian/commit/6f8630f7f1cc141b2f042b31e7a5726b7c8fcc1c))
* **review:** resolve coderabbit findings — azure chunking, epic key, auth helper, a11y, stale integrations ([367340c](https://github.com/Meridiona/meridian/commit/367340c39e52913818f167526818351b9f8e1c24))
* **tasks:** hide provider tabs and tasks for disconnected integrations ([b7df9dc](https://github.com/Meridiona/meridian/commit/b7df9dc192ababdcf5bc1e0b224709424a2df1cf))
* **tasks:** set cwd to ~/.meridian on tasks-sync spawn and refresh board on focus ([d233a7e](https://github.com/Meridiona/meridian/commit/d233a7e63d3b4b60cb5850f424ecc500255e63ca))
* **tasks:** show 'no tasks assigned' hint when connected but board is empty ([3cada0d](https://github.com/Meridiona/meridian/commit/3cada0dd5bc93e0de25e78b617313283b34428a0))

### 🤖 CI

* **ui:** run bun test in the UI job ([9cd9951](https://github.com/Meridiona/meridian/commit/9cd995125dc73b05e18e753de7b4f128f801be75))

### ✅ Tests

* **linear:** add 16 tests for upsert/prune field mapping and filtering ([58f911a](https://github.com/Meridiona/meridian/commit/58f911a83c1da1d2144b019a3c261029be68198f))
* **providers:** cover epic/parent linkage for azure-devops and jira ([f1aed97](https://github.com/Meridiona/meridian/commit/f1aed976aa45cdb1c1bffff0410a54fa9fe77514))
* **tasks:** add regression tests for disconnected-provider filtering ([b1d8862](https://github.com/Meridiona/meridian/commit/b1d8862d2eaff2601fb6beff089a13a78bbd1722))

### 📝 Documentation

* document MLX runtime publish flow for services/ changes ([6cebd60](https://github.com/Meridiona/meridian/commit/6cebd60d64f6bbbf3f6eddf5b01ff04161cd6f4e))

## [1.66.2](https://github.com/Meridiona/meridian/compare/v1.66.1...v1.66.2) (2026-06-27)

### 🐛 Bug Fixes

* **prefetch:** disable Xet + bounded retry-with-resume to stop 4h hangs ([4778e7a](https://github.com/Meridiona/meridian/commit/4778e7a36e8d878aceb727d96cf84999ca879963))
* **prefetch:** harden retry loop per self-review ([#350](https://github.com/Meridiona/meridian/issues/350)) ([4f4b944](https://github.com/Meridiona/meridian/commit/4f4b944c59c937553279d48cbfadd76403818a6c))

## [1.66.1](https://github.com/Meridiona/meridian/compare/v1.66.0...v1.66.1) (2026-06-26)

### 🐛 Bug Fixes

* **prefetch:** replace module-level globals with _speed_state dict ([bfc01e6](https://github.com/Meridiona/meridian/commit/bfc01e693ff0e59eabe09dce387be0f74564bbc6)), closes [#347](https://github.com/Meridiona/meridian/issues/347)
* **setup:** address remaining CodeRabbit findings on PR [#347](https://github.com/Meridiona/meridian/issues/347) ([af2ced2](https://github.com/Meridiona/meridian/commit/af2ced2f2c7c1031928bb06a3ff097dd030f915a))

## [1.66.0](https://github.com/Meridiona/meridian/compare/v1.65.0...v1.66.0) (2026-06-26)

### 🚀 Features

* **onboarding:** minimal model step, early background download, real progress ([adc739c](https://github.com/Meridiona/meridian/commit/adc739c6f61e6cea042d2db4cf920b7a4ecf6e9e))
* **onboarding:** provision all three models (llm + reranker + embedder) end-to-end ([546ca09](https://github.com/Meridiona/meridian/commit/546ca09fc04dcef034569d35c4761808745b5bbc))

### 🐛 Bug Fixes

* **onboarding:** address code review findings on model provisioning PR ([07f3514](https://github.com/Meridiona/meridian/commit/07f3514ffa974be5ed766bbc7d10d4c63ac3de26))
* **onboarding:** derive download speed from byte deltas (hf_xet bypasses tqdm) ([bb0656a](https://github.com/Meridiona/meridian/commit/bb0656aedf4ac54b1202248c9647371e700c5208))
* **onboarding:** use hf-xet for Xet-backed models, wire HF_TOKEN ([a93e87c](https://github.com/Meridiona/meridian/commit/a93e87ce860b87c97b4103b47ff3ff0cb13526a4))

### 📝 Documentation

* **contributing:** document re-triggering the onboarding wizard in dev ([91dbcd4](https://github.com/Meridiona/meridian/commit/91dbcd4454858092537bfcc623bdd27f09a100b7))

### 🔧 Chores

* **mlx:** fix stale run_task_linker_mlx log prefix in mlx_classifier ([9dbe661](https://github.com/Meridiona/meridian/commit/9dbe661a2b7294141be0144f80a374823593326c))

## [1.65.0](https://github.com/Meridiona/meridian/compare/v1.64.0...v1.65.0) (2026-06-26)

### 🚀 Features

* **integrations:** in-process OAuth + in-app token connect for all 5 trackers ([9e341e2](https://github.com/Meridiona/meridian/commit/9e341e2d72cb98fda3b170ef3774ae464173d526))
* **intelligence:** gate task-linking + worklog on a working pipeline ([#147](https://github.com/Meridiona/meridian/issues/147)) ([22e74aa](https://github.com/Meridiona/meridian/commit/22e74aa0534b4ec5ca6bd2bb2e296dfc14733dca))
* **oauth:** browser OAuth for Jira, tasks sync, health improvements ([e757977](https://github.com/Meridiona/meridian/commit/e7579770878c66f46542dd21b97dc39379276d29)), closes [#213](https://github.com/Meridiona/meridian/issues/213)
* **observability:** auto-sync OO dashboards on git push ([0fabfa1](https://github.com/Meridiona/meridian/commit/0fabfa1bff93b13608f4b264f5d7dffed9f00055))
* **setup:** remove llm model selector, fix single fixed model ([d1d09f1](https://github.com/Meridiona/meridian/commit/d1d09f1f649862d87208479e4d07b74846b48a47))
* **ui:** dashboard full-screen with dock integration + go-to-setup button ([7e1389e](https://github.com/Meridiona/meridian/commit/7e1389e0c75d5f2d4a03315ea43a3dd4574266c3))
* **worklog-pipeline:** restore missing service files from cleanup branch ([b6763a3](https://github.com/Meridiona/meridian/commit/b6763a38c3d7a726c464f1f1a7fab32cd79fd228))
* **worklog-pipeline:** restore worklog_pipeline package from cleanup branch ([cf525ba](https://github.com/Meridiona/meridian/commit/cf525ba1b3b291b764f842210a673f2b6d3f41bb))

### 🐛 Bug Fixes

* **capture:** fall back to OCR when browser a11y tree is chrome-only ([#346](https://github.com/Meridiona/meridian/issues/346)) ([60cb64e](https://github.com/Meridiona/meridian/commit/60cb64e7790a9aad951e73ac6e7ac2d3a80f3d6d)), closes [#1](https://github.com/Meridiona/meridian/issues/1) [#2](https://github.com/Meridiona/meridian/issues/2) [#3](https://github.com/Meridiona/meridian/issues/3) [#4](https://github.com/Meridiona/meridian/issues/4) [#5](https://github.com/Meridiona/meridian/issues/5) [#6](https://github.com/Meridiona/meridian/issues/6) [#7](https://github.com/Meridiona/meridian/issues/7) [#9](https://github.com/Meridiona/meridian/issues/9) [#10](https://github.com/Meridiona/meridian/issues/10)
* **ci:** drop install.sh from screenpipe MIT pin check + fix markdownlint ([a3884df](https://github.com/Meridiona/meridian/commit/a3884dff57e5f8d40f5a18f32a70b7da4e622357))
* **cli:** add tray log targets to `meridian logs` ([f71d14a](https://github.com/Meridiona/meridian/commit/f71d14aec43474dd2ff8dd363bdb1a045b4c3705))
* cut staging.2 for live auto-update test [staging-release] ([390e48f](https://github.com/Meridiona/meridian/commit/390e48f7b8a3a4f9877c36da1a3e2a7aece6134b))
* **dev:** clean startup, worklog pipeline wiring, and a11y capture gate ([b72ed4b](https://github.com/Meridiona/meridian/commit/b72ed4b6572a1d94c0de3daf1f470dbb7c935932))
* **dev:** fix popover 404 under tauri dev by copying tray/src into ui/public/popover ([72a7aa3](https://github.com/Meridiona/meridian/commit/72a7aa38878084fdad3576402dc0ed3960717871))
* **dev:** harden popover copy — rm -rf before cp, tighten regression test ([6007672](https://github.com/Meridiona/meridian/commit/6007672e9cae3cdf885a41c148d04485614fff78))
* **dev:** skip screenpipe permission prompts in install-dev.sh ([1cedc54](https://github.com/Meridiona/meridian/commit/1cedc54675b77c6052a9832afee3ee6bd23b3618))
* **etl:** suppress VS Code frames when focused terminal is a coding agent ([#345](https://github.com/Meridiona/meridian/issues/345)) ([d183a8c](https://github.com/Meridiona/meridian/commit/d183a8ce048f8669221dfa1c2dca22f6afbf5f75)), closes [#1](https://github.com/Meridiona/meridian/issues/1) [#2](https://github.com/Meridiona/meridian/issues/2) [#3](https://github.com/Meridiona/meridian/issues/3) [#4](https://github.com/Meridiona/meridian/issues/4) [#5](https://github.com/Meridiona/meridian/issues/5) [#7](https://github.com/Meridiona/meridian/issues/7) [#9](https://github.com/Meridiona/meridian/issues/9) [#10](https://github.com/Meridiona/meridian/issues/10) [#8](https://github.com/Meridiona/meridian/issues/8)
* **install:** skip permissions walkthrough by default in --dev mode ([6edb079](https://github.com/Meridiona/meridian/commit/6edb079c82eedcee154316dd4a8d1252e3af432e))
* **install:** stage screenpipe binary to ~/.meridian/bin for stable TCC path ([#209](https://github.com/Meridiona/meridian/issues/209)) ([5b9aff3](https://github.com/Meridiona/meridian/commit/5b9aff3b97858cf601b5a699ff40d91932693084))
* **integrations:** address deferred review findings from [#338](https://github.com/Meridiona/meridian/issues/338) ([899b75d](https://github.com/Meridiona/meridian/commit/899b75d5661519d8d78b5a351a4d2ecbaf986420))
* **integrations:** address PR [#338](https://github.com/Meridiona/meridian/issues/338) review findings ([897b099](https://github.com/Meridiona/meridian/commit/897b0994a948ff7f456caafd4f1cf4c93806b7bf))
* **integrations:** make token-connected Jira disconnectable; scope Jira self-hosted copy honestly ([721b52b](https://github.com/Meridiona/meridian/commit/721b52b0110f236a6813c3da4697eefcc71ac1c6))
* **integrations:** write .env with trailing newline so appended keys don't concatenate ([d5ded63](https://github.com/Meridiona/meridian/commit/d5ded634c240c5ceaee6eacb8fc714b21f4fedaa))
* **mlx-server:** add missing instrument_agno() to observability module ([4daba64](https://github.com/Meridiona/meridian/commit/4daba641be9069e2260ed5b90dce8c0330f88bd8))
* **oauth:** remove authorize URL from stderr to clear CodeQL taint path ([1c9a541](https://github.com/Meridiona/meridian/commit/1c9a541018df4a62f6ff7c573c3465ceede7aa36))
* **observability:** sanitise agent_name before using it as a log filename ([2320bf6](https://github.com/Meridiona/meridian/commit/2320bf62b49e488b583a921b07ab42abe586cb12))
* **observability:** structured warning + empty safe_name fallback ([f652739](https://github.com/Meridiona/meridian/commit/f652739ad3a0dbac3626c59d34685f3cc93b8c18))
* **observability:** use SDK propagator in current_traceparent for tracestate support ([f4740c2](https://github.com/Meridiona/meridian/commit/f4740c24316b203f248a7af8ff76e666d7f0a71e))
* **observability:** validate agent_name to prevent path traversal in log file path ([1a66fa7](https://github.com/Meridiona/meridian/commit/1a66fa7215c9d8e5ecfe8ce9420fa5936798cac8))
* remove model selector, fix overdue hygiene, drop stale eval artifacts ([13e6941](https://github.com/Meridiona/meridian/commit/13e6941ed3dc8120c58818ad53a513b8f9ef9a8d))
* **review:** address PR [#341](https://github.com/Meridiona/meridian/issues/341) review comments and CodeQL security alert ([c50107a](https://github.com/Meridiona/meridian/commit/c50107a66dfd9fd969b40462fc7f5f20459243af))
* **review:** address remaining PR [#341](https://github.com/Meridiona/meridian/issues/341) CodeRabbit findings ([49a56c6](https://github.com/Meridiona/meridian/commit/49a56c6c411d4ab95c8f295aaa2764e05c004a4f))
* **review:** address remaining PR [#341](https://github.com/Meridiona/meridian/issues/341) review comments ([4ccd42f](https://github.com/Meridiona/meridian/commit/4ccd42f183bedfadd8642214c7d4edfa9cf42f1f))
* **tray:** revert activation policy on dashboard close + tracing on open_setup ([94a3217](https://github.com/Meridiona/meridian/commit/94a3217027676ee40e3909823b5981a8e95b6655))
* **ui:** handle activation policy Result + move Go to Setup to top of settings ([176353a](https://github.com/Meridiona/meridian/commit/176353a16544dd82565d451762166b469fa760a4))

### ✅ Tests

* auto-update target staging.2 [staging-release] ([b9600ad](https://github.com/Meridiona/meridian/commit/b9600ad71fef050f4a7f915d538eaa27f3b5fbfd))
* **tray:** guard v1.64.0 dev-setup architecture contracts ([0f99905](https://github.com/Meridiona/meridian/commit/0f999055fce68f00330cdd7e7e85a7dbae5b2c59))

### 📝 Documentation

* **claude:** fix stale classify-trigger bullet — pending_classifier → summarised worklog pickup ([7f263c8](https://github.com/Meridiona/meridian/commit/7f263c86f06994518bf1bfa87e608d9e798d996a))
* **claude:** fix stale pending_classifier lifecycle references ([a198dc4](https://github.com/Meridiona/meridian/commit/a198dc4d2a99e5b4b28fbc4481732fefc88ee407))
* **claude:** remove stale coding-agent-classify CLI reference + update MLX server description ([9068c83](https://github.com/Meridiona/meridian/commit/9068c8353f8f8e0edecdf5a36635129be0394457))
* update dev setup + all docs for v1.64.0 architecture ([cbfac64](https://github.com/Meridiona/meridian/commit/cbfac64377e4096ef0ede6a1b7fb4861e60a0f3b))

### 🔧 Chores

* **cleanup:** remove classifier pipeline + modularise MLX server + Qwen3.5-2B ([155b26c](https://github.com/Meridiona/meridian/commit/155b26c5e096c006a7711bec3df795c432afe305))
* **dev:** skip PM tool credential prompts in install-dev.sh ([a11289b](https://github.com/Meridiona/meridian/commit/a11289b28ae52de9efb34002b2878da6e53638ff))
* **dev:** strip Claude Code integrations from install-dev.sh ([31c07b2](https://github.com/Meridiona/meridian/commit/31c07b28a58de2a6af8fc16848e2592dc970fa22))
* **install:** remove screenpipe and a11y-helper from install.sh ([9bc9595](https://github.com/Meridiona/meridian/commit/9bc95950aa0f1ef84615de41ab463ef8dae34feb))
* **merge:** resolve conflicts merging pre-main into feat/in-process-oauth ([e647407](https://github.com/Meridiona/meridian/commit/e6474072812fd69a5af2c391e146cf57940d811a))
* **merge:** resolve conflicts merging pre-main into feat/in-process-oauth ([70c88be](https://github.com/Meridiona/meridian/commit/70c88be892d0e142ac732ab42734f9f2cabc2e3a))
* **release:** first staging cut [staging-release] ([fc691aa](https://github.com/Meridiona/meridian/commit/fc691aa760ac27cab0fe598b15589bbdcd1b740f))
* **release:** retry staging [staging-release] ([be174ba](https://github.com/Meridiona/meridian/commit/be174ba0b5a3853c2908e57e6b135581a174b8d9))
* remove classifier eval scratch files, rerank experiments, and stale tests ([ab8479c](https://github.com/Meridiona/meridian/commit/ab8479c20d8b465eaa56c83a1cea37de73d1d97c))
* **summariser:** drop pending_classifier — summarised rows are terminal ([fa31b30](https://github.com/Meridiona/meridian/commit/fa31b30d975472e870f2d9d45e144f29a8ceec27))

## [1.64.0](https://github.com/Meridiona/meridian/compare/v1.63.0...v1.64.0) (2026-06-24)

### 🚀 Features

* **capture:** a11y-tree text capture with OCR fallback (Gap-2 Bucket 2, slice 3b) ([172d1bf](https://github.com/Meridiona/meridian/commit/172d1bf80469506b4891069d052031f52ed8e883))
* **capture:** capture_ui_events table + writer (Gap-2 Bucket 2, slice 3c part 1) ([22d2f2b](https://github.com/Meridiona/meridian/commit/22d2f2b47be005a99a9a2fcba4effd0fc6f78996))
* **capture:** in-process input recorder → capture_ui_events (Gap-2 Bucket 2, slice 3c part 2) ([a246310](https://github.com/Meridiona/meridian/commit/a2463101986a71e117fc5fc661293df47891b4e8))
* **capture:** persist captured frames to meridian.db (Gap-2 Bucket 2, slice 4a) ([6e4f816](https://github.com/Meridiona/meridian/commit/6e4f816773b9b2a4d6f3f7de51e1793047ace5eb))
* **ci:** build the self-contained MLX runtime tarball on an arm64 runner (Approach C, Step 2) ([dc1bd56](https://github.com/Meridiona/meridian/commit/dc1bd56dde5069d5743dca14a9bd672e4b8eb750))
* **cli:** implement `meridian uninstall` (Gap-2 Bucket 1, teardown) ([036c59a](https://github.com/Meridiona/meridian/commit/036c59af9d68a1e398d750f108e35803153e92ce))
* **core:** log which settings.json path wins at resolution time ([8817b05](https://github.com/Meridiona/meridian/commit/8817b0536c1b87d8a887abad9c671c75e9ba078c))
* **core:** move settings reader to meridian-core; port /api/settings GET ([640a352](https://github.com/Meridiona/meridian/commit/640a3521655c06cc08dc23e49091db2ce9aded73))
* **core:** port /api/active view to Rust; Sidebar reads it in-app ([8a402d9](https://github.com/Meridiona/meridian/commit/8a402d92e0e9541a737cfab405d86fc5b7c084fe))
* **core:** port /api/coding-agents to Rust (get_coding_agents command) ([12c34ce](https://github.com/Meridiona/meridian/commit/12c34ce29c71566777ea0e2d187f0137d283e002))
* **core:** port /api/plan GET+POST to Rust (board scoring + writes) ([7745324](https://github.com/Meridiona/meridian/commit/77453247d2865d6c24102d2b60e73006e6077720))
* **core:** port /api/plan/task single-ticket detail to Rust ([a7ed6c3](https://github.com/Meridiona/meridian/commit/a7ed6c385358edca71446495eb83972f03bbc5dc))
* **core:** port /api/settings PUT to Rust (atomic settings write) ([1d85b3a](https://github.com/Meridiona/meridian/commit/1d85b3afda2825a8b345f5f224c3c69ea5dd38ca))
* **core:** port /api/tasks (+ hygiene) to Rust; 4 consumers read it in-app ([97e5ea0](https://github.com/Meridiona/meridian/commit/97e5ea0dc96393aaa1441a8cd2387a9733477e4a))
* **core:** port /api/today to Rust — byte-identical to the Node route ([e2c708e](https://github.com/Meridiona/meridian/commit/e2c708eada572f8f58a11c141e7b7eda741bae45))
* **core:** port /api/week to Rust (golden-compare passes) ([c8204a8](https://github.com/Meridiona/meridian/commit/c8204a8275d217fa99271d21a63c2f03d5991286))
* **core:** port /api/worklogs read to Rust; WorklogsView reads it in-app ([81d2f4b](https://github.com/Meridiona/meridian/commit/81d2f4b82ce94a189421482866d44c6436878ca6))
* **core:** port localDayBounds/todayString to meridian-core (date-utils) ([6004e6f](https://github.com/Meridiona/meridian/commit/6004e6fd87351a4f81654d4f9a2ce07f40e47569))
* **core:** port notification delivery writes; de-HTTP the poll loop ([7f70fa3](https://github.com/Meridiona/meridian/commit/7f70fa3549b3df94ee4c89db82adf773181062a4))
* **core:** port notifications read policy + pending queue to Rust ([391af51](https://github.com/Meridiona/meridian/commit/391af515e1c94958e666b45ab7cf79aa88ba92db))
* **core:** port the dashboard interval math to meridian-core ([564d15a](https://github.com/Meridiona/meridian/commit/564d15a9e520510609bd09117c67eace7cea8762))
* **core:** port triage decision + ignore writes to Rust ([8b72ade](https://github.com/Meridiona/meridian/commit/8b72ade025aa7a305b668eefdca677eab599e38b))
* **core:** port worklog review writes + notice-clear DELETE to Rust ([c8739be](https://github.com/Meridiona/meridian/commit/c8739be401363572b0ba8107b6f1ee267752a34b))
* **etl:** cut the daemon ETL over to in-process capture tables (Gap-2 Bucket 2, slice 4b-1) ([9cee2a2](https://github.com/Meridiona/meridian/commit/9cee2a2a58ba0e4f986015351948c8f477975ca4))
* **fold:** static-export the dashboard into Tauri, delete /api (cutover) ([5e5e174](https://github.com/Meridiona/meridian/commit/5e5e174a8510928003f15f98472d84bd60e36a12))
* **health:** retire screenpipe from the daemon; repoint health to in-process capture (Gap-2 Bucket 2, slice 4b-2) ([4ebc7e8](https://github.com/Meridiona/meridian/commit/4ebc7e82967f74e64e6887a8419956f9b9cd40c2))
* **integrations:** clear provider tasks from DB on disconnect ([9d6bc74](https://github.com/Meridiona/meridian/commit/9d6bc74825a8a01f0700ba8b128f8ca9143ece13))
* **integrations:** connect GitHub via gh CLI browser OAuth ([dfb2003](https://github.com/Meridiona/meridian/commit/dfb200318c2a9a1235d1f2eda70221d042339458))
* **mlx-runtime:** add a staging release channel (runtime-staging-v* → runtime-staging) ([ae6a31b](https://github.com/Meridiona/meridian/commit/ae6a31b8a1946f40387013cb846d5948e790218a))
* **mlx-runtime:** wire the manifest URL + publish a rolling runtime-latest release ([349c14c](https://github.com/Meridiona/meridian/commit/349c14c095990f37fafce77c676ed7e8d1011109))
* **obs:** close the logs/traces gaps across the ported read paths ([bd71b22](https://github.com/Meridiona/meridian/commit/bd71b22ebf28313f976560279ab54383bd83bbf0))
* **obs:** debug-level per-op flow across every meridian-core read path ([2791540](https://github.com/Meridiona/meridian/commit/27915409f3b34f3de5f7ff971a4f6e1f0ec67d20))
* **obs:** trace the dashboard read path into OpenObserve (dev otel feature) ([455a337](https://github.com/Meridiona/meridian/commit/455a337584aaa2b06a817a93ad64679d4005f2f3))
* **obs:** trace the today read path into OpenObserve ([6bf209b](https://github.com/Meridiona/meridian/commit/6bf209bb1e323486088ea5bea32c316549a0acfc))
* port /api/triage GET + /api/triage/parents to Rust ([784b48d](https://github.com/Meridiona/meridian/commit/784b48d324f80405f7750003bcde6aaa5869311c))
* **setup:** add Input Monitoring permission card to the wizard (Gap-2 Bucket 2, Fix 2) ([33a8404](https://github.com/Meridiona/meridian/commit/33a840430f0d1eb1e92f2e6048153770d199a12e))
* **setup:** eager spec-aware model download in the wizard (Gap-2 Bucket 2, Fix 3) ([3c9756b](https://github.com/Meridiona/meridian/commit/3c9756be302eaf6f0d2c2ea6e48c0338731cd4ff))
* **setup:** port "A · Rail" first-run wizard with on-device model picker ([d073b37](https://github.com/Meridiona/meridian/commit/d073b3714833e09908d35e0895af48148d092384))
* **setup:** register Input Monitoring via IOHIDRequestAccess (Gap-2 Bucket 2, Fix B) ([1a6dfee](https://github.com/Meridiona/meridian/commit/1a6dfee17f94bdfbb5b96eb15c97f315930a39a2))
* **tauri:** in-app native dashboard window (Today/Week from Rust, no browser) ([d2d63a7](https://github.com/Meridiona/meridian/commit/d2d63a7a8662ca66a77e8671abb320fc4d10db03))
* **tauri:** pooled meridian.db state + active-session panel (runtime proof) ([686b816](https://github.com/Meridiona/meridian/commit/686b816315651fd263407383efe39af2fee85bbf)), closes [#db-active](https://github.com/Meridiona/meridian/issues/db-active)
* **tauri:** reuse meridian lib in the tray to read meridian.db ([02b49db](https://github.com/Meridiona/meridian/commit/02b49db63004b552295410e2211b5a1a98ce9826))
* **tray:** background runtime auto-upgrade via stage-and-swap (Gap-2 follow-up b) ([e267939](https://github.com/Meridiona/meridian/commit/e267939115a291962f88d041ba2e7aaf72d41526))
* **tray:** bundle the non-capture backend into the .app (Gap-2 Bucket 1, slice 1) ([cdff263](https://github.com/Meridiona/meridian/commit/cdff26310e2fa4ac65678fbe4c5f6036edc8e2d1))
* **tray:** compile-time runtime-manifest channel override (option_env!) ([a983713](https://github.com/Meridiona/meridian/commit/a983713cc3b5ad7b7eee9f8828915dc8fc867d1a))
* **tray:** console log subscriber for capture runtime checks (Bucket 2) ([4c185e1](https://github.com/Meridiona/meridian/commit/4c185e16ca8bd63b99f39abb25cd290718a8b6cd))
* **tray:** DMG auto-update with in-app update banner ([8ee1340](https://github.com/Meridiona/meridian/commit/8ee1340fc45d8c5df3379d21518c4a2656ff6b7f))
* **tray:** first-run install of the bundled backend (Gap-2 Bucket 1, slice 1b) ([56f7468](https://github.com/Meridiona/meridian/commit/56f746887e6d7cb30924459f15ab4b023cb81ec2))
* **tray:** in-app onboarding wizard skeleton + window-capability fix ([7b61929](https://github.com/Meridiona/meridian/commit/7b61929131163a663adc64292e6a40a81cef9388))
* **tray:** in-process capture — boundary + screenpipe-screen engine (Gap-2 Bucket 2, slices 1-2) ([b3877f8](https://github.com/Meridiona/meridian/commit/b3877f87ff477a6a0119edae1dd7a456e9d8398c))
* **tray:** live menu-bar pill with progress ring + task key ([d577ee8](https://github.com/Meridiona/meridian/commit/d577ee87102ff4bb433cefcb07678ff0c4a96a4b))
* **tray:** log which source wins DB path resolution in meridian_db_path ([05a5823](https://github.com/Meridiona/meridian/commit/05a5823c367e948a757b574960c665fcde1ca3ab))
* **tray:** Next fold stage 0 — devUrl→next dev, /setup is first Next window ([497be87](https://github.com/Meridiona/meridian/commit/497be87bb57e29ae12c7e061283a9bc025b08ac5))
* **tray:** port /api/integrations GET; TasksView reads it in-app ([244b644](https://github.com/Meridiona/meridian/commit/244b6449004400e98d4809be751a9bd621fbd08a))
* **tray:** port /api/tasks/sync to Rust (board re-sync spawn) ([8113431](https://github.com/Meridiona/meridian/commit/8113431d8efe4994e57fea01475c6ba6f62f3997))
* **tray:** port /api/triage/apply to Rust (hygiene fix write-back) ([824985c](https://github.com/Meridiona/meridian/commit/824985c4921c4f9678f0da588892ae480e07b91e))
* **tray:** port /api/update to Rust (launch updater in Terminal) ([593fb02](https://github.com/Meridiona/meridian/commit/593fb0221c610712915e9bf27b29054a067bfa57))
* **tray:** port /api/version; Sidebar reads it in-app (+ tokio process fix) ([d358e3b](https://github.com/Meridiona/meridian/commit/d358e3b8fbeeb69e103346d00bd310460576ae38))
* **tray:** port daemon/reload + openobserve POST (finish Settings Apply) ([2cea948](https://github.com/Meridiona/meridian/commit/2cea9480f53e72230eac2ed7cc0cf0d004b8e943))
* **tray:** port daemon/status, health, openobserve GET, logs GET to Rust ([076b156](https://github.com/Meridiona/meridian/commit/076b1563b477a7264ab5359ab43f27f2bbc2a539))
* **tray:** port integrations connect/disconnect to Rust ([d1e61d7](https://github.com/Meridiona/meridian/commit/d1e61d7b97b4675c83e5996865613bf88a42ed17))
* **tray:** port the 4 SSE streams to Tauri events (Next-fold) ([4c914bc](https://github.com/Meridiona/meridian/commit/4c914bc820eff2b584a31acb1158f6b9bdb7d4df))
* **tray:** rebrand app to "Meridian" and use the brand mark as the icon ([207eb64](https://github.com/Meridiona/meridian/commit/207eb648569103b2d2ad839dfc24ac2d39df1c59))
* **tray:** redesign popover and show it on tray left-click ([e4cb601](https://github.com/Meridiona/meridian/commit/e4cb601c0cdbf80c1ca571bca4269deb80a6990b))
* **tray:** rich hover-tooltip card on tray icon enter/leave ([a1f5cef](https://github.com/Meridiona/meridian/commit/a1f5cef01138d818b59796d76c4da84b10ce0b7b))
* **tray:** supervise the MLX server from the poll loop (auto-restart on death) ([67b49a0](https://github.com/Meridiona/meridian/commit/67b49a0e98241912eb86fbb375916736e439ffb6))
* **tray:** switch MLX packaging to Approach C (download-and-provision) ([d15913c](https://github.com/Meridiona/meridian/commit/d15913ce1cfb04bd29e2269f2928dd336157adaa))
* **tray:** verify + version-check the MLX runtime download (Approach C, Step 3) ([fa19404](https://github.com/Meridiona/meridian/commit/fa194046f076158428ed9f10d46d8c30460c1d8b))
* **tray:** window-aware capture — per-window app/window/url metadata (Bucket 2, slice 3a) ([9780215](https://github.com/Meridiona/meridian/commit/97802159f2de5e0ac7a86c5eeeb9faeb808af442))
* **tray:** wire onboarding wizard — MLX server manager, setup commands, first-run auto-open ([4d6b1f6](https://github.com/Meridiona/meridian/commit/4d6b1f667d088eee02b76e55bb06839beee74b30))
* **ui:** fold — WeekView renders from Rust get_week in-app ([7d88788](https://github.com/Meridiona/meridian/commit/7d8878897d624dcb2f930b6ca6c97379c278351a))
* **ui:** Next fold stage 1 — dual-path bridge; /today renders from Rust ([265bede](https://github.com/Meridiona/meridian/commit/265bedeacf2c6ffe43b744f912b142f604bdb66f))

### 🐛 Bug Fixes

* **build:** sync Cargo.lock with the block2 tray dep ([4ee8a51](https://github.com/Meridiona/meridian/commit/4ee8a51c976e0ef331fcb902d62ac83f4b4f1b18))
* **capture:** preflight each permission before prompting (Gap-2 Bucket 2) ([b3beecb](https://github.com/Meridiona/meridian/commit/b3beecbd8cc52fd3439e44dd1a5d092820dbf906))
* **core:** address adityaharishch code-review findings [#1](https://github.com/Meridiona/meridian/issues/1)–[#7](https://github.com/Meridiona/meridian/issues/7) ([ea19768](https://github.com/Meridiona/meridian/commit/ea19768d78c767fef1527a2549920f752aff0603)), closes [#2](https://github.com/Meridiona/meridian/issues/2) [#3](https://github.com/Meridiona/meridian/issues/3) [#4](https://github.com/Meridiona/meridian/issues/4) [#5](https://github.com/Meridiona/meridian/issues/5) [#6](https://github.com/Meridiona/meridian/issues/6)
* **core:** address code-review findings [#4](https://github.com/Meridiona/meridian/issues/4)/[#6](https://github.com/Meridiona/meridian/issues/6)/[#8](https://github.com/Meridiona/meridian/issues/8)/[#10](https://github.com/Meridiona/meridian/issues/10) ([ac70eca](https://github.com/Meridiona/meridian/commit/ac70eca6754e50a06c260ffbaae0fcaddbd9bdd6))
* **core:** readers query coding_agent_session_uuid, not claude_session_uuid ([fa74b47](https://github.com/Meridiona/meridian/commit/fa74b47135702f4c1e09181ac1e78b6c02663755))
* **core:** today active-session silently dropped (column guard read wrong table) ([276537c](https://github.com/Meridiona/meridian/commit/276537cdf335c6e12c20a665fcbdd4c7598ff4e1))
* cut staging.2 for live auto-update test [staging-release] ([9c3fde8](https://github.com/Meridiona/meridian/commit/9c3fde8e34ad916f895d14f1d5093ccf68bb9e82))
* **daemon:** correct the MLX-server restart command in user-facing error text ([56e5099](https://github.com/Meridiona/meridian/commit/56e5099bb82b99e75f76ab833c42e8a90908a9a6))
* **daemon:** silence classifier-offline notice during onboarding; drop dev fix text (Gap-2 Bucket 2, Fix A) ([491479f](https://github.com/Meridiona/meridian/commit/491479f1d599af10f832a0abd03e09bd199495e0))
* **db:** set busy_timeout on the daemon's meridian.db pool (Gap-2 Bucket 2, review fix) ([75b6633](https://github.com/Meridiona/meridian/commit/75b6633f37c7fe219ee8741914f5dc7c32661036)), closes [#324](https://github.com/Meridiona/meridian/issues/324)
* **fold:** correct four Next-fold route divergences from code review ([b54ec04](https://github.com/Meridiona/meridian/commit/b54ec044b47a27a885594b6f90c97d718ff685ae)), closes [#298](https://github.com/Meridiona/meridian/issues/298)
* **health:** drop vestigial disk (screenpipe) check (Gap-2 Bucket 2) ([b845647](https://github.com/Meridiona/meridian/commit/b84564702db97c8116e6d95b271acbfb81dfc233))
* **health:** resolve all pr [#330](https://github.com/Meridiona/meridian/issues/330) review findings ([41d2aa2](https://github.com/Meridiona/meridian/commit/41d2aa2520edae85c86cd2c65c1fb75b5056c5ac))
* **install:** clean up legacy bundle agents + migrate .env on DMG upgrade ([a782319](https://github.com/Meridiona/meridian/commit/a782319c4754a39591b493a39b781043c5db2cb9))
* **installer:** write credentials to ~/.meridian/.env not ~/.meridian/app/.env ([cecc8aa](https://github.com/Meridiona/meridian/commit/cecc8aa5808bf2035375a2f0d2e9d8fbbadc1fd3))
* **install:** purge leftover pre-cutover screenpipe agent on in-process install (Gap-2 Bucket 2) ([0c36a55](https://github.com/Meridiona/meridian/commit/0c36a558dddec124ae514fdeae09d920af8182bf))
* **integrations:** auto-install gh via Homebrew if not present ([f617e99](https://github.com/Meridiona/meridian/commit/f617e99f3243d9b0d8dd1fed620f8325a8682475))
* **integrations:** fix gh CLI path resolution and GITHUB_TOKEN env conflict ([ac987e0](https://github.com/Meridiona/meridian/commit/ac987e0d8700aa6bfe8472e5271f87cd0f8d0f82))
* **oauth:** guard Jira login on missing client secret; scope admin-block hint ([afdeede](https://github.com/Meridiona/meridian/commit/afdeede5caa24263915a05dec4cc9d85a83d3bc6))
* **oauth:** prevent GITHUB_TOKEN from merging onto prior .env line ([f6b8781](https://github.com/Meridiona/meridian/commit/f6b8781b1cae3febefdf50d6c44933c46eb3530a))
* **obs:** demote per-payload telemetry spool logs to TRACE ([6209b2e](https://github.com/Meridiona/meridian/commit/6209b2e88ffb41a6d450247e8b508d3201e369de))
* **obs:** init OTLP inside the Tokio runtime (fixes no-reactor panic) ([32fec44](https://github.com/Meridiona/meridian/commit/32fec445f2fe13bfb2858a89968749562c4e012f))
* **review:** address PR [#324](https://github.com/Meridiona/meridian/issues/324) review findings across capture, tray, setup wizard ([7732a82](https://github.com/Meridiona/meridian/commit/7732a8270a8e4be57280a8755d73f1d78f156ba7))
* **setup:** address code-review findings on the wizard ([b24031b](https://github.com/Meridiona/meridian/commit/b24031b5229b2018c5565727c82588d079268233))
* **setup:** call CGRequestScreenCaptureAccess so Meridian appears in the list ([7a1b155](https://github.com/Meridiona/meridian/commit/7a1b155d86e96573c87c2138d1d5608a43f7221f))
* **setup:** degrade model prefetch gracefully on an older runtime (Gap-2 Bucket 2, Fix C-a) ([100876a](https://github.com/Meridiona/meridian/commit/100876a6b3ae6fc0c69ecf22977fac4f2e5f136f))
* **setup:** detect Screen Recording via CGPreflight, not pgrep screenpipe (Gap-2 Bucket 2) ([3119f74](https://github.com/Meridiona/meridian/commit/3119f74855241ea3b80d63ebe02bd6d3af2f6c95))
* **today:** clamp presence so a stale active block can't inflate focus ([0a62663](https://github.com/Meridiona/meridian/commit/0a626634fff6e3526246902af0e7b116b15ba4c7))
* **tray:** address code-review findings for PR [#327](https://github.com/Meridiona/meridian/issues/327) ([5ad4e24](https://github.com/Meridiona/meridian/commit/5ad4e24c23ae6e72327d8e90054c3fe28b6c80f6))
* **tray:** convert popover+tooltip to NSPanel for proper fullscreen Space support ([74af02b](https://github.com/Meridiona/meridian/commit/74af02b996d16ce055f5587511abd9f0974ee765))
* **tray:** enable native fullscreen and resize on dashboard window ([f0f3d71](https://github.com/Meridiona/meridian/commit/f0f3d71aa9e8f47e108d88b5177cb23f0e363455))
* **tray:** global NSEvent monitor for click-outside + restore non-activating NSPanel ([2a420db](https://github.com/Meridiona/meridian/commit/2a420db88f0ba0985c51fe4a03acc8561b7e0853))
* **tray:** grant window set-size so the popover can resize (uncut) ([ce72a64](https://github.com/Meridiona/meridian/commit/ce72a64c540ce1e5778a82a7ce58cf9762da5ed7))
* **tray:** manual popover positioning below menu bar + remove dead positioner call ([bfb8030](https://github.com/Meridiona/meridian/commit/bfb80303660fee3c79fe1ff6bceb178725ce05fe))
* **tray:** popover bottom corners clipped + scrollbar ([00d4781](https://github.com/Meridiona/meridian/commit/00d4781f5d8633f90f2c9594cafc4086635b5666))
* **tray:** popover visible on all macOS Spaces (full-screen apps) ([6f752da](https://github.com/Meridiona/meridian/commit/6f752da13d212cf99562b063147866a0f0571090))
* **tray:** position popover via tray rect instead of TrayCenter plugin ([a1b7e65](https://github.com/Meridiona/meridian/commit/a1b7e65866eaaa84583980914b37e78fa447d3b9))
* **tray:** raise window level to NSPopUpMenuWindowLevel + orderFrontRegardless for fullscreen ([f231f68](https://github.com/Meridiona/meridian/commit/f231f686cde24a941be7ecbc6b0c965f47afc17c))
* **tray:** remove 8px gap between menu bar and hover tooltip ([a6e9d95](https://github.com/Meridiona/meridian/commit/a6e9d95b4a4a2c4965ad058a8e2dc8e87ef02458))
* **tray:** remove native OS tooltip from tray icon ([e8222fc](https://github.com/Meridiona/meridian/commit/e8222fc25fb9c7d022e91b09ba4b528fbdeb38a1))
* **tray:** restore click-outside dismiss — drop non-activating NSPanel mask ([8b01b39](https://github.com/Meridiona/meridian/commit/8b01b397885f18268ef51ad075c972b3ef32feb4))
* **tray:** ship the spirograph brand mark, not the ring-dot placeholder [staging-release] ([dd393e2](https://github.com/Meridiona/meridian/commit/dd393e2e52ffd3bfa842261556e6a8af295f32b9))
* **tray:** show popover + tooltip over fullscreen apps ([e30fbe9](https://github.com/Meridiona/meridian/commit/e30fbe9798192dcdb6284d426e4f68dba056d6cc))
* **tray:** single source of truth for the tray menu (stop dropping items) ([2acec6b](https://github.com/Meridiona/meridian/commit/2acec6b64c28557e5929aff0e2ccd2200ec6f453))
* **tray:** startup flash, screen-recording prompt loop, and popover height ([d406f32](https://github.com/Meridiona/meridian/commit/d406f32c27f3387e689a73221085cadfd007b6cd))
* **tray:** stop dev-watcher thrash from next dev's .next churn ([3dfe86f](https://github.com/Meridiona/meridian/commit/3dfe86f93a276911614da82718adc9815ef466b9))
* **tray:** stop forcing ad-hoc signing so TCC grants persist across builds ([6dd81d5](https://github.com/Meridiona/meridian/commit/6dd81d559257e1cd665336740b8e89e306d54665))
* **tray:** tooltip border-clip + useful no-task state ([e8e1a4b](https://github.com/Meridiona/meridian/commit/e8e1a4bfc2f154484b73441d81be12a09d9bb8fe))
* **tray:** use orderFrontRegardless for popover so it shows in fullscreen Space ([8fb8420](https://github.com/Meridiona/meridian/commit/8fb84204c7030e2f378690228c649443ddb110c9))
* **tray:** wire Jira OAuth — forward client_secret, surface errors fast ([1cbec2e](https://github.com/Meridiona/meridian/commit/1cbec2e09febd0d6ae113bbab2b680ce8999c7db))

### ♻️ Refactoring

* **core:** extract meridian-core lean shared data layer ([f4002ea](https://github.com/Meridiona/meridian/commit/f4002ea073726d24863a4d0f806fa70b7eca0484))
* **core:** organize meridian-core into readers/ + util/ folders ([48f5e1c](https://github.com/Meridiona/meridian/commit/48f5e1c454a2a19cc9ccfb4a0a62a57e0cc25c5d))
* **tray:** group commands under commands/ + extract sys/install/tray ([f53448f](https://github.com/Meridiona/meridian/commit/f53448fbe3c059e7c45d95402b699689e0fc87e3))
* **tray:** make the poll loop HTTP-free (cutover prerequisite) ([6eeba10](https://github.com/Meridiona/meridian/commit/6eeba10a866e1e2c732f99965384438f5ec8a4d7))
* **tray:** move canonical .env to ~/.meridian/.env, drop app/ fallback ([b0d8f73](https://github.com/Meridiona/meridian/commit/b0d8f7394db76b648271e1c8110942e78547f37f))
* **tray:** single detect_install_mode() probe for env file resolution ([e9006fc](https://github.com/Meridiona/meridian/commit/e9006fc6987b7275066c852b4bcbb0f69b35bb0b))
* **tray:** split poll.rs into poll/ folder ([8f9fb23](https://github.com/Meridiona/meridian/commit/8f9fb2387cbdbfa6cca74ab3b53659597cdf8f6f))
* **ui:** make the bridge Tauri-only (cutover step 1) ([22b7061](https://github.com/Meridiona/meridian/commit/22b7061111a499b1e05405f78108995c0f4d427e))
* **ui:** relocate API response types to lib/api-types (cutover step 2) ([0940250](https://github.com/Meridiona/meridian/commit/0940250a82f98d7671467dc169304bca3195131b))

### 📦 Build System

* **release:** detailed changelog showing all commit types ([d12cfa6](https://github.com/Meridiona/meridian/commit/d12cfa60c4be270e528bb2f62b9ec04c8595a5d5))
* **release:** wire DMG auto-update into the semantic-release pipeline ([5383f43](https://github.com/Meridiona/meridian/commit/5383f43a64a449a3ce708daf53f34f92e67d0199))
* **tray:** ad-hoc sign the .app + DMG so unsigned builds aren't 'damaged' ([e1d0230](https://github.com/Meridiona/meridian/commit/e1d023069ae44de00a4dd56a5760a7c584aa7a90))
* **tray:** enable in-process capture by default (Gap-2 Bucket 2) ([a01ad01](https://github.com/Meridiona/meridian/commit/a01ad017749580f92fc0c721fb45eefb1b377d62))
* **tray:** pin staging runtime manifest via build:staging (Gap-2 Bucket 2, Fix 1) ([34edc5b](https://github.com/Meridiona/meridian/commit/34edc5b6d8cc96776b771a8437759e919c7ebf63))
* **tray:** rebuild the daemon as part of build/build:staging (Gap-2 Bucket 2) ([3d3634b](https://github.com/Meridiona/meridian/commit/3d3634bdaf5c6aaf333c40fdb8235425c222a4a2))
* **tray:** stable dev code-signing so TCC grants persist across rebuilds (Gap-2 Bucket 2) ([5573294](https://github.com/Meridiona/meridian/commit/5573294988f9254c99085db2c77aff17bf799a4a))

### 🤖 CI

* authenticate the private screenpipe-fork git dep in the Rust job ([e55bf31](https://github.com/Meridiona/meridian/commit/e55bf3102a036f384fcfc90040862504ab9d846d))
* **mlx-runtime:** bump actions to current majors (checkout@v7, upload@v7, download@v8) ([8685bef](https://github.com/Meridiona/meridian/commit/8685bef36f8dee936e465d5317c0a6beb9fbff24))
* **mlx-runtime:** smoke-test the tarball across all hosted arm64 macOS versions ([f5533aa](https://github.com/Meridiona/meridian/commit/f5533aa4c822678907cccc3494b2d15f9616260f))
* **release:** add DMG auto-update staging channel on pre-main ([ba6d539](https://github.com/Meridiona/meridian/commit/ba6d5394174411429b9f9be7a0ce4e13ff90e8e5))
* **release:** authenticate CI git to the private screenpipe-fork dep ([0a5a726](https://github.com/Meridiona/meridian/commit/0a5a72655a21a59a049ebde5f9e7d071dd43d501))
* **release:** authenticate production build to the private screenpipe-fork dep ([118cb6f](https://github.com/Meridiona/meridian/commit/118cb6f5bc25b26206d0232a6f537a969490797f))
* **release:** drop the orphaned Node 22 cache step (Stage 5) ([21c793e](https://github.com/Meridiona/meridian/commit/21c793e638700d88e96347fdd8a85ee627267ab1))
* **release:** trigger staging on pre-main marker-commit + dispatch, not on main ([5555a2f](https://github.com/Meridiona/meridian/commit/5555a2f831180917d9f9034ea6f862c184587ba9))

### ✅ Tests

* auto-update target staging.2 [staging-release] ([82e90ad](https://github.com/Meridiona/meridian/commit/82e90ad138863533f21e31a1abe93208bcf3f074))
* **core:** add get_today perf bench; flag the migration in meridian-ui skill ([65b4aae](https://github.com/Meridiona/meridian/commit/65b4aae050f6c7a284ae97c40a78ee810a39b5b8))
* **core:** unit-test hygiene parsing + integration-test the DB readers ([8db7115](https://github.com/Meridiona/meridian/commit/8db71154eb255d06c6823e114af33e0d7736eac1))
* **tray:** live end-to-end pull of the runtime-staging channel ([01b9e7b](https://github.com/Meridiona/meridian/commit/01b9e7b460f1adb5f93e490510b646c2629d647e))
* **tray:** static regression guards + diagnostics for popover/tooltip ([f2ad2ae](https://github.com/Meridiona/meridian/commit/f2ad2ae2e994a63379fe7dd2ba39c6a61133fe4b))
* **updater:** add real-GitHub dry-run helper ([57b9d5f](https://github.com/Meridiona/meridian/commit/57b9d5f2ef083ebe9c743d5de826ccb7405b0466))

### 📝 Documentation

* **capture:** document the screenpipe→in-process cutover + reword stale health text (Gap-2 Bucket 2) ([49ad45f](https://github.com/Meridiona/meridian/commit/49ad45f1f51de848797b586e8886769aa8972523))
* **claude:** add the Next-fold porting playbook as the dev standard ([3776302](https://github.com/Meridiona/meridian/commit/37763029b7a909eb2985173d3d58815215c05d91))
* **claude:** document meridian-core + the dashboard→Tauri migration ([dee4564](https://github.com/Meridiona/meridian/commit/dee45648e6c2f529f58e8f1e888a85020fe8ee9b))
* **claude:** mark the Next-fold cutover as landed ([affe038](https://github.com/Meridiona/meridian/commit/affe038db9dd610d4502da4021e25fe0ce9e781c))
* **claude:** update repo layout + Next-fold playbook for new module structure ([e2cda24](https://github.com/Meridiona/meridian/commit/e2cda24102d53b996c986d5268c5255de6df9348))
* **core:** backfill module docs to the triage.rs bar (crate-consistent) ([9595158](https://github.com/Meridiona/meridian/commit/9595158fbcd9c596311825835956a2bdba665701))
* dashboard is in the tray app, not a localhost server (Stage 5) ([9ecd81a](https://github.com/Meridiona/meridian/commit/9ecd81ab0ee33ef061ab10ebcb847c199dcb9de9))
* **obs:** add OBSERVABILITY.md — navigating the DB→UI flow in OpenObserve ([a2eeb9f](https://github.com/Meridiona/meridian/commit/a2eeb9fef99081d1c53a7b2063b5662d677cfaa9))

### 🎨 Styles

* **daemon:** cargo fmt src/main.rs ([8be137f](https://github.com/Meridiona/meridian/commit/8be137f6699eae9b14b4bafe92d65e0898f345bb))

### 🔧 Chores

* **deps:** pin screenpipe to the last MIT release (0.4.6) + CI guard ([12f1214](https://github.com/Meridiona/meridian/commit/12f1214a13b9f1030525bf1efce0ae73f74f5790))
* merge main + cargo fmt the new files it brought in ([cf731d6](https://github.com/Meridiona/meridian/commit/cf731d681d47ac97d8a55e9fede6951b1444435b))
* **merge:** resolve conflicts merging pre-main into spike/meridian-core ([a4eb72b](https://github.com/Meridiona/meridian/commit/a4eb72be0a9cb4377c4cbe1d07d46140b9b6d665))
* **release:** first staging cut [staging-release] ([7a232c5](https://github.com/Meridiona/meridian/commit/7a232c520ecf06abff4b60748203969638ffa032))
* **release:** retire the standalone dashboard Node server (Stage 5) ([272c59c](https://github.com/Meridiona/meridian/commit/272c59cd416c980aa5b42f5bdf1392cba6cea95d))
* **release:** retry staging [staging-release] ([31f4095](https://github.com/Meridiona/meridian/commit/31f4095c9c021cc7fa682574df12b820dab5f956))
* **runtime:** bump services to 1.60.0 to republish staging runtime with prefetch endpoints (Gap-2 Bucket 2, Fix C-b) ([8942252](https://github.com/Meridiona/meridian/commit/894225234e8a25f66177ed431dc28721e2bd8f29))
* sync services/uv.lock to meridian-agents 1.56.0 ([eb51d24](https://github.com/Meridiona/meridian/commit/eb51d245cac00c8b56c22669553b8210f06da19e))
* **tray:** remove superseded vanilla wizard + dashboard pages ([de53148](https://github.com/Meridiona/meridian/commit/de53148e81ea3ecb305193b025bd2a8d5271001b))
* **ui:** drop Node-server-only deps after the fold (Stage 5) ([0f134e6](https://github.com/Meridiona/meridian/commit/0f134e602a5083ba7dba2dd8c285a037942da1d8))
* **ui:** gitignore out/, remove dead Node libs after the cutover ([6d57fff](https://github.com/Meridiona/meridian/commit/6d57ffff9b993d22d6726b050128d3f47422cc69))

# [1.63.0](https://github.com/Meridiona/meridian/compare/v1.62.0...v1.63.0) (2026-06-23)


### Features

* **etl:** noise filter — clean session_text at ingest time ([#325](https://github.com/Meridiona/meridian/issues/325)) ([7b56e66](https://github.com/Meridiona/meridian/commit/7b56e66db4323b6eb55b81aa8b71be38de682614))

# [1.62.0](https://github.com/Meridiona/meridian/compare/v1.61.1...v1.62.0) (2026-06-19)


### Features

* **observability:** summariser continuity dashboard + install-time auto-import ([#316](https://github.com/Meridiona/meridian/issues/316)) ([0ba45e1](https://github.com/Meridiona/meridian/commit/0ba45e1c5fe1687b70a41fcc12025466ebea5740))

## [1.61.1](https://github.com/Meridiona/meridian/compare/v1.61.0...v1.61.1) (2026-06-18)


### Bug Fixes

* **mlx-server:** serialize model calls + raise worklog timeout + fix coding metrics rows ([#313](https://github.com/Meridiona/meridian/issues/313)) ([dd2a043](https://github.com/Meridiona/meridian/commit/dd2a0436195698c9eff2a2ba31b73587abf165ac))

# [1.61.0](https://github.com/Meridiona/meridian/compare/v1.60.0...v1.61.0) (2026-06-18)


### Features

* **cleanup:** mark overdue tickets as done or cancelled from the cleanup page ([#312](https://github.com/Meridiona/meridian/issues/312)) ([5998324](https://github.com/Meridiona/meridian/commit/5998324c5d4447e057fcdb530e90bbe880138356))

# [1.60.0](https://github.com/Meridiona/meridian/compare/v1.59.0...v1.60.0) (2026-06-18)


### Features

* **classifier:** TASK GATE prompt + DeepEval untracked-guard eval (70%→96% guard) ([#310](https://github.com/Meridiona/meridian/issues/310)) ([9fad7b5](https://github.com/Meridiona/meridian/commit/9fad7b5d7cfabc6b315b72bf07a98110ef7cf2cd))

# [1.59.0](https://github.com/Meridiona/meridian/compare/v1.58.1...v1.59.0) (2026-06-17)


### Features

* **classifier:** plan-only candidate set + 30-min per-ticket continuity context ([#309](https://github.com/Meridiona/meridian/issues/309)) ([f19d42c](https://github.com/Meridiona/meridian/commit/f19d42c16152ae9ed70615364cb53b3046d40b96))

## [1.58.1](https://github.com/Meridiona/meridian/compare/v1.58.0...v1.58.1) (2026-06-17)


### Bug Fixes

* **install:** revive screenpipe after `meridian stop` (enable before bootstrap + retry) ([#308](https://github.com/Meridiona/meridian/issues/308)) ([e277bbb](https://github.com/Meridiona/meridian/commit/e277bbb0e577c4fe2a87a285899b421627a3b920))

# [1.58.0](https://github.com/Meridiona/meridian/compare/v1.57.0...v1.58.0) (2026-06-17)


### Features

* **observability:** land stranded [#305](https://github.com/Meridiona/meridian/issues/305) trace/dashboard work + rename claude_session_uuid → coding_agent_session_uuid ([#306](https://github.com/Meridiona/meridian/issues/306)) ([1f51e1d](https://github.com/Meridiona/meridian/commit/1f51e1d853ccdb8bad9e5d61625275c44cff71ee))

# [1.57.0](https://github.com/Meridiona/meridian/compare/v1.56.0...v1.57.0) (2026-06-17)


### Features

* **observability:** one root trace per classified session + frame-range attribution ([#304](https://github.com/Meridiona/meridian/issues/304)) ([0d76152](https://github.com/Meridiona/meridian/commit/0d76152fb06e58823ef2c408c166ec5e582d87dd))
* **today:** replace AI-assisted % with coding time + fix Shape of Day ([#307](https://github.com/Meridiona/meridian/issues/307)) ([d79b33b](https://github.com/Meridiona/meridian/commit/d79b33b8a23000c3e76f1d2998a766fdcbfd2d03))

# [1.56.0](https://github.com/Meridiona/meridian/compare/v1.55.0...v1.56.0) (2026-06-17)


### Bug Fixes

* **plan:** retain completed planned tasks + surface overdue tickets in clean-up ([#301](https://github.com/Meridiona/meridian/issues/301)) ([f6ea76a](https://github.com/Meridiona/meridian/commit/f6ea76a8e36f1313d628b99214d7d047819a3723))
* **worklog:** make synth emit JSON, not prose (PII hook + json_schema) ([#300](https://github.com/Meridiona/meridian/issues/300)) ([47f389f](https://github.com/Meridiona/meridian/commit/47f389f4e65fd09cf1c18e8294aa627de9fb2339)), closes [#297](https://github.com/Meridiona/meridian/issues/297)


### Features

* **observability:** durable telemetry spool + full worklog lineage trace ([#299](https://github.com/Meridiona/meridian/issues/299)) ([bafd96b](https://github.com/Meridiona/meridian/commit/bafd96beee2f6ee961b167dc1bfce5b60cb911ad))

# [1.55.0](https://github.com/Meridiona/meridian/compare/v1.54.1...v1.55.0) (2026-06-17)


### Features

* **observability:** full session-task classifier debugging in OpenObserve ([#297](https://github.com/Meridiona/meridian/issues/297)) ([72598f0](https://github.com/Meridiona/meridian/commit/72598f0d016d5a35a7339f10fcd8ce7ce36e0fcb))

## [1.54.1](https://github.com/Meridiona/meridian/compare/v1.54.0...v1.54.1) (2026-06-16)


### Bug Fixes

* **mlx:** address code-review findings on idle-evict ([67f7fa1](https://github.com/Meridiona/meridian/commit/67f7fa1434c2555daa365bb0da7269cc881c81af))


### Performance Improvements

* **mlx:** idle-evict the classifier model to free ~7 GB when idle ([3f7bbe7](https://github.com/Meridiona/meridian/commit/3f7bbe76d34bf685abd233e37061b51693aab8e1))

# [1.54.0](https://github.com/Meridiona/meridian/compare/v1.53.1...v1.54.0) (2026-06-16)


### Features

* land notification system + classifier plan-boost onto main ([#294](https://github.com/Meridiona/meridian/issues/294)) ([d0c5d3f](https://github.com/Meridiona/meridian/commit/d0c5d3fe461a5211961e6756fe0924960c836da9)), closes [#289](https://github.com/Meridiona/meridian/issues/289) [#290](https://github.com/Meridiona/meridian/issues/290) [#288](https://github.com/Meridiona/meridian/issues/288) [#289](https://github.com/Meridiona/meridian/issues/289) [#290](https://github.com/Meridiona/meridian/issues/290)

## [1.53.1](https://github.com/Meridiona/meridian/compare/v1.53.0...v1.53.1) (2026-06-16)


### Bug Fixes

* **dev:** resolve meridian binary to repo build in dev to stop migration drift ([#291](https://github.com/Meridiona/meridian/issues/291)) ([088d5da](https://github.com/Meridiona/meridian/commit/088d5da593197e1556b7f162745e181310d56835))
* **dev:** stop the previous dev run in dev-start.sh (no more piled-up daemons) ([#293](https://github.com/Meridiona/meridian/issues/293)) ([3a8756b](https://github.com/Meridiona/meridian/commit/3a8756bf3d0a404f8974154eb621ef5ee7c9a049))

# [1.53.0](https://github.com/Meridiona/meridian/compare/v1.52.6...v1.53.0) (2026-06-16)


### Features

* **ui:** daily "today's plan" page with declared working set ([#288](https://github.com/Meridiona/meridian/issues/288)) ([b2c724d](https://github.com/Meridiona/meridian/commit/b2c724db7f49ed04b1740a3905fd351e682b7e23))

## [1.52.6](https://github.com/Meridiona/meridian/compare/v1.52.5...v1.52.6) (2026-06-15)


### Bug Fixes

* **ui:** spawn native meridian binary, not the node wrapper, under launchd ([#283](https://github.com/Meridiona/meridian/issues/283)) ([ad1504c](https://github.com/Meridiona/meridian/commit/ad1504ca5a520bd0e00bc848c0214d7c06f338d0))

## [1.52.5](https://github.com/Meridiona/meridian/compare/v1.52.4...v1.52.5) (2026-06-15)


### Bug Fixes

* **openobserve:** correct ZO_MEMORY_CACHE_MAX_SIZE and ZO_DATAFUSION_POOL_SIZE units ([#284](https://github.com/Meridiona/meridian/issues/284)) ([1e9e111](https://github.com/Meridiona/meridian/commit/1e9e111933180893dd4075027885a7052efc620b))

## [1.52.4](https://github.com/Meridiona/meridian/compare/v1.52.3...v1.52.4) (2026-06-14)


### Bug Fixes

* **uninstall:** tear down orphaned OpenObserve + legacy crash-loop agents ([1ce31a1](https://github.com/Meridiona/meridian/commit/1ce31a1d3a2f3b3de9d0f9f0e5988c9cf6bd5103)), closes [#158](https://github.com/Meridiona/meridian/issues/158)

## [1.52.3](https://github.com/Meridiona/meridian/compare/v1.52.2...v1.52.3) (2026-06-14)


### Bug Fixes

* **dev:** give install-dev.sh full parity with the production install ([669d37a](https://github.com/Meridiona/meridian/commit/669d37a26fc39714766355ae16be093685e51395))
* **dev:** guard session-summary cp so a missing source can't abort the dev install ([03b1f1d](https://github.com/Meridiona/meridian/commit/03b1f1df57b99e94db00441a81a850dffb736d14))
* **ui:** gate standalone output on build phase so dev server boots ([353700e](https://github.com/Meridiona/meridian/commit/353700eaae56a11baf62509360b40b6f5a7534f0)), closes [vercel/next.js#87881](https://github.com/vercel/next.js/issues/87881)

## [1.52.2](https://github.com/Meridiona/meridian/compare/v1.52.1...v1.52.2) (2026-06-14)


### Bug Fixes

* **ui:** render display serif in Georgia to match design ([681f387](https://github.com/Meridiona/meridian/commit/681f38702fb5232552cde6571731d5a97a7aacdb))

## [1.52.1](https://github.com/Meridiona/meridian/compare/v1.52.0...v1.52.1) (2026-06-13)


### Bug Fixes

* **bundle:** ship the OpenObserve installer + plist in the release package ([b218c56](https://github.com/Meridiona/meridian/commit/b218c560b8739b03c8b3c21204b29f4dee70efcd))

# [1.52.0](https://github.com/Meridiona/meridian/compare/v1.51.0...v1.52.0) (2026-06-13)


### Bug Fixes

* **config:** resolve settings.json from a fixed, install-independent path ([b8a1f2a](https://github.com/Meridiona/meridian/commit/b8a1f2adfe3badcbffa9e6abd253564082ca86a6))
* **observability:** empty MERIDIAN_OTLP_ENDPOINT no longer disables export; drop RUST_LOG from daemon plist ([a1b2f1b](https://github.com/Meridiona/meridian/commit/a1b2f1b8a57770377c85d26a106851af7364f473))
* **ui:** detect post-init OpenObserve credential mismatch; surface error text ([6a6bb5b](https://github.com/Meridiona/meridian/commit/6a6bb5bed9e51f3e84f130e82d35a45b92a904e8))
* **ui:** don't bounce a running OpenObserve on Apply; surface start failures ([9de6e6d](https://github.com/Meridiona/meridian/commit/9de6e6dd28b9004616353c3ad6c0c974ebcfe6a4))
* **ui:** restore lost Radix switch styles; clarify Log Level scope ([049a0d0](https://github.com/Meridiona/meridian/commit/049a0d00d2ffeb60a326636fffe77a416b3ce95b))


### Features

* **observability:** make settings.json the source for OpenObserve credentials ([0a20319](https://github.com/Meridiona/meridian/commit/0a203196cfb8f3bc3c92b9b44bb6ef5e1393e36f))
* **observability:** OpenObserve service runs only when export is enabled ([48c9b50](https://github.com/Meridiona/meridian/commit/48c9b5099a478d09d31aff28febc50d162054730))
* **ui:** bootstrap OpenObserve from the toggle on a fresh machine ([5bb2833](https://github.com/Meridiona/meridian/commit/5bb283320fc1806e1fb301b4d0319b3e2b4fea01)), closes [#271](https://github.com/Meridiona/meridian/issues/271)
* **ui:** first-time OpenObserve credential setup + endpoint as advanced field ([99f6cf6](https://github.com/Meridiona/meridian/commit/99f6cf6157d5f4f9951a4a907ae72adfa36f9e51))
* **ui:** gate OpenObserve settings behind a top-level enable toggle ([8f36b7c](https://github.com/Meridiona/meridian/commit/8f36b7c235c3852e747f162cc66d247d5d504b8d))

# [1.51.0](https://github.com/Meridiona/meridian/compare/v1.50.1...v1.51.0) (2026-06-12)


### Bug Fixes

* **triage:** harden curation persistence against silent data loss ([8d04264](https://github.com/Meridiona/meridian/commit/8d042643bfd0298db19a72debc45599c92c8755e))
* **ui:** add Clean-up to the real sidebar nav (not Nav.tsx) ([232be4e](https://github.com/Meridiona/meridian/commit/232be4e3d24a06efb2913c15b4b8a60eaabc7662))


### Features

* **triage:** deterministic ticket-triage engine for onboarding board cleanup ([2f99632](https://github.com/Meridiona/meridian/commit/2f99632f7e6e94dbf9bdeb4534590b68b990b84f))
* **triage:** flag tickets missing a due date (board-guarded) ([df4bb04](https://github.com/Meridiona/meridian/commit/df4bb041abedea157ba5855ce4778864a3c967ac))
* **triage:** full Definition-of-Ready ruleset + per-rule fix model ([d6a4a74](https://github.com/Meridiona/meridian/commit/d6a4a741170dfc2885a14839f1f87d1bb1f9aa7d))
* **triage:** persist triage verdicts, run on sync, gate classification ([dde65c2](https://github.com/Meridiona/meridian/commit/dde65c29fafb9931afc8090dec03a09a9a9ad3ec))
* **triage:** treat far-future due dates as not-current work ([c093b14](https://github.com/Meridiona/meridian/commit/c093b146d39b1a848773f93f56e293d392bf832d))
* **triage:** write board-hygiene fixes back to the real tracker ([fefb681](https://github.com/Meridiona/meridian/commit/fefb68177772df69a1917a7c9504fd42a9243f7a))
* **ui:** board-hygiene fix dialog from the Tasks view ([1c14dfa](https://github.com/Meridiona/meridian/commit/1c14dfabf7f2491669bbe2eeac0ad81df9506abc))
* **ui:** onboarding board-cleanup screen + triage API ([653b2a7](https://github.com/Meridiona/meridian/commit/653b2a71ae73ac7eea48bb65a7b1d62dd8d45dd7))
* **ui:** redesign cleanup page + per-issue ignore (must-fix can't be ignored) ([a536802](https://github.com/Meridiona/meridian/commit/a5368020bde295dc08a7f6007ecd3549e1b93731))
* **ui:** severity-tiered cleanup — must-fix banner + cleanup page ([fac0348](https://github.com/Meridiona/meridian/commit/fac0348ba5fce1183a8a36ce47cc4782c5cb2a25))
* **ui:** surface board-hygiene fixes inline in the Tasks view ([5c1ef25](https://github.com/Meridiona/meridian/commit/5c1ef25dd9707faa83a4cd06c1ee5c9a22e88d4d))

## [1.50.1](https://github.com/Meridiona/meridian/compare/v1.50.0...v1.50.1) (2026-06-12)


### Bug Fixes

* **tasks:** sort task sessions in descending order by start time ([#270](https://github.com/Meridiona/meridian/issues/270)) ([c807305](https://github.com/Meridiona/meridian/commit/c807305dddb879b84bce22529096485769d172f6))

# [1.50.0](https://github.com/Meridiona/meridian/compare/v1.49.2...v1.50.0) (2026-06-11)


### Features

* **observability:** system notices fault bus, log viewer UI, and UI auth for all providers ([#266](https://github.com/Meridiona/meridian/issues/266)) ([916a6d2](https://github.com/Meridiona/meridian/commit/916a6d20118da290fc8be64026d608f6d366ebb0))

## [1.49.2](https://github.com/Meridiona/meridian/compare/v1.49.1...v1.49.2) (2026-06-11)


### Bug Fixes

* **dev:** suppress update-available banner in dev mode ([#265](https://github.com/Meridiona/meridian/issues/265)) ([77e50b4](https://github.com/Meridiona/meridian/commit/77e50b40a1eefec24fe2d0abd4722037a5d0e2c8))

## [1.49.1](https://github.com/Meridiona/meridian/compare/v1.49.0...v1.49.1) (2026-06-11)


### Bug Fixes

* **dev:** correct stale summary text printed after install-dev.sh ([#264](https://github.com/Meridiona/meridian/issues/264)) ([147425d](https://github.com/Meridiona/meridian/commit/147425df369a36d435f2acb4c4f688677a57f1cd))

# [1.49.0](https://github.com/Meridiona/meridian/compare/v1.48.2...v1.49.0) (2026-06-11)


### Features

* **dev:** add hot-reload dev environment with dev-start.sh ([5280dbb](https://github.com/Meridiona/meridian/commit/5280dbbbf058f9f2dfbeff4fc1c64d2a66c3f509))

## [1.48.2](https://github.com/Meridiona/meridian/compare/v1.48.1...v1.48.2) (2026-06-11)


### Bug Fixes

* **github:** guard against null nodes in ProjectV2 GraphQL response ([8098c60](https://github.com/Meridiona/meridian/commit/8098c60b25077bb0f46393a7c7500e9dd41ab585))
* **pm:** make task status dynamic across all providers ([aff118f](https://github.com/Meridiona/meridian/commit/aff118f769d239d371bf2e656fa0c935e9111ecb))
* **pm:** renumber migration to 036, fmt, and type alias ([82e352d](https://github.com/Meridiona/meridian/commit/82e352d3e58eb069e5c5dd565c1bb585f6a56c17)), closes [#258](https://github.com/Meridiona/meridian/issues/258)
* **pm:** update eval dataset builder for dynamic status ([41b8746](https://github.com/Meridiona/meridian/commit/41b8746332a75e9830fe418cd8932a7bd0863580))
* **status:** word-boundary keyword matching + once_cell env cache ([92b8901](https://github.com/Meridiona/meridian/commit/92b89014405ac96e56dde2c6dbb0da2a53eff3c5))

## [1.48.1](https://github.com/Meridiona/meridian/compare/v1.48.0...v1.48.1) (2026-06-11)


### Bug Fixes

* **db:** self-heal migration checksums instead of crash-looping ([190a52f](https://github.com/Meridiona/meridian/commit/190a52f71d9e38e51f3dfbebdb09bf7d31dba5c3))

# [1.48.0](https://github.com/Meridiona/meridian/compare/v1.47.0...v1.48.0) (2026-06-11)


### Bug Fixes

* **tasks:** address PR review — fix epic map key, isDueSoon, dates formatting, startDate removal ([f461d50](https://github.com/Meridiona/meridian/commit/f461d500f16a573824cb00e1370f5f016613fcda))


### Features

* **daemon:** SIGHUP-triggered restart + UI Apply button with polling ([f7e4cbb](https://github.com/Meridiona/meridian/commit/f7e4cbbe9e5e69f39fad2804aea41693c4c0e2f4))
* **observability:** configure OpenObserve OTLP export from UI settings ([01ff003](https://github.com/Meridiona/meridian/commit/01ff003ee1b4fbe883114f48f4e02ed98f9b082b))
* **observability:** hot-reload log level without daemon restart ([1162866](https://github.com/Meridiona/meridian/commit/1162866e394f44c5ada96c3ac6ff5acfee7fad86))
* **tasks:** fetch and store due_date/start_date from Jira and Linear ([d983355](https://github.com/Meridiona/meridian/commit/d983355b8c6a107cc8c61ed91e6a9a6fb02a6b71))
* **ui/tasks:** epic grouping, collapsible sections, due dates, sticky detail ([1dead7f](https://github.com/Meridiona/meridian/commit/1dead7fa2b6dbb60ad88d5c4679ab208e37e92a1))

# [1.47.0](https://github.com/Meridiona/meridian/compare/v1.46.0...v1.47.0) (2026-06-11)


### Bug Fixes

* **favicon:** add 48px frame to favicon.ico (16/32/48) ([a2b7766](https://github.com/Meridiona/meridian/commit/a2b77666a350bcd75343295cda6ec010bb3b3355))
* **favicon:** compress icon.png 139 KB → 52 KB with pngquant --quality=80-95 ([842490b](https://github.com/Meridiona/meridian/commit/842490b02c51b82a66786bf6e7e1a3308f534012))
* **observability:** address PR [#251](https://github.com/Meridiona/meridian/issues/251) security and validation review comments ([662908e](https://github.com/Meridiona/meridian/commit/662908e793dcd8a7455e1517f6c198f8ad5e90fc)), closes [#3](https://github.com/Meridiona/meridian/issues/3) [#256](https://github.com/Meridiona/meridian/issues/256) [#4](https://github.com/Meridiona/meridian/issues/4)
* **ui:** add cursor:pointer to all interactive buttons across the product ([552d354](https://github.com/Meridiona/meridian/commit/552d354d3301485c7bd69b79225eb8eb6873076b))
* **ui:** correct cursor semantics — not-allowed for disabled, remove redundant inline rules ([fbe4858](https://github.com/Meridiona/meridian/commit/fbe485866474c9f9fc81e05fac1b1b24ba573ee1))


### Features

* **observability:** configure OpenObserve OTLP export from UI settings ([b7f9aa1](https://github.com/Meridiona/meridian/commit/b7f9aa134053ccc3346bb05ba2d43eb42b3429d7))
* **ui:** add favicon from Meridiona tray icon ([6ef13ed](https://github.com/Meridiona/meridian/commit/6ef13ed8dcd8b13bdb19661b136511bd808c5dd2))

# [1.46.0](https://github.com/Meridiona/meridian/compare/v1.45.3...v1.46.0) (2026-06-11)


### Features

* **ui:** show task title and PM-tool link on worklog cards ([#252](https://github.com/Meridiona/meridian/issues/252)) ([4ecbafa](https://github.com/Meridiona/meridian/commit/4ecbafad59fccded31ed6e0fed586f486e6aeebb))


### Reverts

* **ci:** restore single-stage release workflow ([787991e](https://github.com/Meridiona/meridian/commit/787991eaa920cfccd1ae714db9d2ea55a1204a8b))

# [1.43.0](https://github.com/Meridiona/meridian/compare/v1.42.0...v1.43.0) (2026-06-10)


### Features

* **azure-devops:** surface PAT permission errors in the UI ([#241](https://github.com/Meridiona/meridian/issues/241)) ([f426420](https://github.com/Meridiona/meridian/commit/f4264208f3c0647ca1c9b571b06b47f2825c3ee8))
* **ui:** show issue type in Tasks page for all providers ([#242](https://github.com/Meridiona/meridian/issues/242)) ([206dca5](https://github.com/Meridiona/meridian/commit/206dca537d2fda07f2784b2372d5135b660bcc76))

# [1.42.0](https://github.com/Meridiona/meridian/compare/v1.41.0...v1.42.0) (2026-06-10)


### Features

* **ui:** show absolute time on hover in Shape of Day activity breakdown ([82a7b89](https://github.com/Meridiona/meridian/commit/82a7b8919287cb660574d0c951711dafabd5ae77))
* **ui:** vendor badges and integrations back nav on Tasks page ([a69ae3c](https://github.com/Meridiona/meridian/commit/a69ae3c1a45d053f4f1d368c59d65037adc7d139))

# [1.41.0](https://github.com/Meridiona/meridian/compare/v1.40.1...v1.41.0) (2026-06-10)


### Features

* **ui:** remove Queue view from navigation ([a5e1592](https://github.com/Meridiona/meridian/commit/a5e159205cd55a83292ea970be53b540ce3d9b15))

## [1.40.1](https://github.com/Meridiona/meridian/compare/v1.40.0...v1.40.1) (2026-06-10)


### Bug Fixes

* **azure-devops:** correct pm_tasks column names in upsert query ([c34ef21](https://github.com/Meridiona/meridian/commit/c34ef216950d467338a1d0d6f691429c9f74a793))
* **azure-devops:** store browser URL instead of REST API URL ([14425c3](https://github.com/Meridiona/meridian/commit/14425c3cda978241da2b74a9967f7ac70b94c4a7))

# [1.40.0](https://github.com/Meridiona/meridian/compare/v1.39.2...v1.40.0) (2026-06-10)


### Bug Fixes

* **install:** make meridian update atomic — stage and swap instead of rm-rf in place ([#236](https://github.com/Meridiona/meridian/issues/236)) ([e8599af](https://github.com/Meridiona/meridian/commit/e8599af7714118a32108361c9cd7315aeae75947))


### Features

* **ui:** group Today task buckets by integration, show titles & time-chunked summaries ([#235](https://github.com/Meridiona/meridian/issues/235)) ([753eaa0](https://github.com/Meridiona/meridian/commit/753eaa0edd2553fb60d79c9c36437e1f642849d3)), closes [Meridiona/meridian#194](https://github.com/Meridiona/meridian/issues/194) [#123](https://github.com/Meridiona/meridian/issues/123)

## [1.39.2](https://github.com/Meridiona/meridian/compare/v1.39.1...v1.39.2) (2026-06-10)


### Bug Fixes

* **azure-devops:** correct pm_tasks column names in upsert query ([#234](https://github.com/Meridiona/meridian/issues/234)) ([20ef4e7](https://github.com/Meridiona/meridian/commit/20ef4e7b39a4a7aeb6f704a2bce9c549e410c325))

## [1.39.1](https://github.com/Meridiona/meridian/compare/v1.39.0...v1.39.1) (2026-06-10)


### Bug Fixes

* **bundle:** include lib-azure-setup.sh in release package ([#233](https://github.com/Meridiona/meridian/issues/233)) ([20a2178](https://github.com/Meridiona/meridian/commit/20a21780917c0d14f6500e318f499c083f4bc718))
* **daemon:** create meridian.db before the MLX preflight and survive MLX being down ([#232](https://github.com/Meridiona/meridian/issues/232)) ([a285232](https://github.com/Meridiona/meridian/commit/a285232a70bec95752ecc884a1d042b6e7c58d37))

# [1.39.0](https://github.com/Meridiona/meridian/compare/v1.38.0...v1.39.0) (2026-06-10)


### Bug Fixes

* **install:** pin Python services to a uv-managed arm64 interpreter ([#228](https://github.com/Meridiona/meridian/issues/228)) ([c327c7b](https://github.com/Meridiona/meridian/commit/c327c7b68f3afb391baf2f3bca9f2cd2f7c226a6))
* **integrations:** simplify Azure DevOps setup to 2 vars + fix visualstudio.com URL parsing ([746cc85](https://github.com/Meridiona/meridian/commit/746cc850df5aafa88b5d42db14f7912f675e31e6))
* **ui:** stop the dashboard crash-looping under a mismatched Node after `meridian update` ([#223](https://github.com/Meridiona/meridian/issues/223)) ([a44d68c](https://github.com/Meridiona/meridian/commit/a44d68cc59e914b5789f7dba80f2c842b404ba9f))


### Features

* **integrations:** add Azure DevOps (VSTS) PM connector ([2636578](https://github.com/Meridiona/meridian/commit/2636578a3bc02a89ad60c01541eb4d10458a458f)), closes [Meridian#42](https://github.com/Meridian/issues/42)

# [1.38.0](https://github.com/Meridiona/meridian/compare/v1.37.4...v1.38.0) (2026-06-09)


### Features

* **setup:** auto-install + sign in to gh when configuring GitHub (no PAT) ([#226](https://github.com/Meridiona/meridian/issues/226)) ([7499ea5](https://github.com/Meridiona/meridian/commit/7499ea5a1fe75e0a9245c2451e212055edd4835b))

## [1.37.4](https://github.com/Meridiona/meridian/compare/v1.37.3...v1.37.4) (2026-06-09)


### Bug Fixes

* **setup:** show live MLX model download progress ([#225](https://github.com/Meridiona/meridian/issues/225)) ([268dff0](https://github.com/Meridiona/meridian/commit/268dff016246e15ab146031b807fb1161ce26383))

## [1.37.3](https://github.com/Meridiona/meridian/compare/v1.37.2...v1.37.3) (2026-06-09)


### Bug Fixes

* **tray:** use launchctl bootstrap instead of deprecated load ([#224](https://github.com/Meridiona/meridian/issues/224)) ([1c1f171](https://github.com/Meridiona/meridian/commit/1c1f171335e25d47a3cf27850cdf5622c143e4ec))

## [1.37.2](https://github.com/Meridiona/meridian/compare/v1.37.1...v1.37.2) (2026-06-09)


### Bug Fixes

* **uninstall:** clean up venv, node-runtime, oauth tokens, and misc files ([#222](https://github.com/Meridiona/meridian/issues/222)) ([162010f](https://github.com/Meridiona/meridian/commit/162010f72cd3f075c3a60d696c42fd76604959aa))

## [1.37.1](https://github.com/Meridiona/meridian/compare/v1.37.0...v1.37.1) (2026-06-09)


### Bug Fixes

* **packaging:** include lib-trello-setup.sh in release bundle ([#221](https://github.com/Meridiona/meridian/issues/221)) ([e5cb6cf](https://github.com/Meridiona/meridian/commit/e5cb6cf3ac0212d2df1ebb0a435c56220e211112))

# [1.37.0](https://github.com/Meridiona/meridian/compare/v1.36.0...v1.37.0) (2026-06-09)


### Features

* **trello:** inject app key at package time, document setup flow ([#220](https://github.com/Meridiona/meridian/issues/220)) ([3182b6a](https://github.com/Meridiona/meridian/commit/3182b6a92f64515fed3277418983446dd089232d))

# [1.36.0](https://github.com/Meridiona/meridian/compare/v1.35.1...v1.36.0) (2026-06-09)


### Bug Fixes

* **oauth:** force recompile when the baked-in client_secret changes ([825f1f5](https://github.com/Meridiona/meridian/commit/825f1f5923fd6dca4b44a83ca36fa98ad43c4b1d))
* **oauth:** send Atlassian client_secret at token exchange ([6a0f3a0](https://github.com/Meridiona/meridian/commit/6a0f3a0278a0c6a4f6854f59d96e765973712898))
* **smoke:** raise classify timeout to 180s for M1 machines ([5c3a62d](https://github.com/Meridiona/meridian/commit/5c3a62d21c656dff0acaca8b91cd93324fc2e63a))
* **uninstall:** also remove downloaded MLX model weights from HF cache ([7f88f81](https://github.com/Meridiona/meridian/commit/7f88f8137862ef54f1edc48596bcf370b4d98e08))
* **uninstall:** remove app bundle, venv, npm package, and hooks on uninstall ([1747183](https://github.com/Meridiona/meridian/commit/17471837aa4a2b3debc75f6064a2628d2168328b))


### Features

* **trello:** full Trello integration — OAuth, task sync, worklog, UI connect/disconnect ([835cd02](https://github.com/Meridiona/meridian/commit/835cd02ea01cc53c8eea7731d6a5859f4a1b8946))

## [1.35.1](https://github.com/Meridiona/meridian/compare/v1.35.0...v1.35.1) (2026-06-09)


### Bug Fixes

* **tray:** rename meridiana-mark icon to meridiona-mark ([211853d](https://github.com/Meridiona/meridian/commit/211853d4dce58f70ffffede4d6739b232f28d13c))

# [1.35.0](https://github.com/Meridiona/meridian/compare/v1.34.6...v1.35.0) (2026-06-09)


### Features

* **oauth:** browser OAuth for Jira, tasks sync, health improvements ([aec7365](https://github.com/Meridiona/meridian/commit/aec7365087caceadb8fa3cf7e6ecd397f2f3fe29)), closes [#213](https://github.com/Meridiona/meridian/issues/213)

## [1.34.6](https://github.com/Meridiona/meridian/compare/v1.34.5...v1.34.6) (2026-06-09)


### Bug Fixes

* **install:** suppress stale mlx-server log lines during update wait ([7fddebe](https://github.com/Meridiona/meridian/commit/7fddebe62f8e3d454f7e585df12ad04451404a6f))

## [1.34.5](https://github.com/Meridiona/meridian/compare/v1.34.4...v1.34.5) (2026-06-09)


### Bug Fixes

* **mlx-server:** remove --backend mlx flag that server.py never accepted ([905fcbe](https://github.com/Meridiona/meridian/commit/905fcbeffa111d97a1e634bea78d90b21eeba9c9))

## [1.34.4](https://github.com/Meridiona/meridian/compare/v1.34.3...v1.34.4) (2026-06-09)


### Bug Fixes

* **install:** preserve existing plist path on update to avoid TCC grant invalidation ([#210](https://github.com/Meridiona/meridian/issues/210)) ([ee4569e](https://github.com/Meridiona/meridian/commit/ee4569ea631b9f1e7edfa8ad24711f559f0581bb))

## [1.34.3](https://github.com/Meridiona/meridian/compare/v1.34.2...v1.34.3) (2026-06-09)


### Bug Fixes

* **installer:** kill tail processes after MLX model-load wait ([#208](https://github.com/Meridiona/meridian/issues/208)) ([079eb11](https://github.com/Meridiona/meridian/commit/079eb118759ada8c14411403603baba551b73f91))
* **install:** stage screenpipe binary to ~/.meridian/bin for stable TCC path ([#209](https://github.com/Meridiona/meridian/issues/209)) ([0a52120](https://github.com/Meridiona/meridian/commit/0a521205a250ebdd3458dbbf59945115fbac6d64))

## [1.34.2](https://github.com/Meridiona/meridian/compare/v1.34.1...v1.34.2) (2026-06-09)


### Bug Fixes

* **uninstall:** remove app bundle, venv, npm package, and hooks on uninstall ([#207](https://github.com/Meridiona/meridian/issues/207)) ([9b5cacf](https://github.com/Meridiona/meridian/commit/9b5cacf7d594a47058f7b39c5a960bd945d6b153))

## [1.34.1](https://github.com/Meridiona/meridian/compare/v1.34.0...v1.34.1) (2026-06-09)


### Bug Fixes

* **screenpipe:** prefer staged Mach-O binary over nvm-versioned npm shim ([#205](https://github.com/Meridiona/meridian/issues/205)) ([23e1db8](https://github.com/Meridiona/meridian/commit/23e1db8cab6470577019cd4fce4bbf048b728c83))
* **smoke:** raise classify timeout to 180s for M1 machines ([#204](https://github.com/Meridiona/meridian/issues/204)) ([b92cce0](https://github.com/Meridiona/meridian/commit/b92cce059135ae16c14af35f031eaedfa653b636))

# [1.34.0](https://github.com/Meridiona/meridian/compare/v1.33.0...v1.34.0) (2026-06-09)


### Features

* **jira:** browser OAuth (PKCE) auth, additive to API-token fallback ([#203](https://github.com/Meridiona/meridian/issues/203)) ([9da291d](https://github.com/Meridiona/meridian/commit/9da291d5d9eadf4f040e0b26a5858092fd6aabdb))

# [1.33.0](https://github.com/Meridiona/meridian/compare/v1.32.1...v1.33.0) (2026-06-09)


### Features

* **installer:** stream MLX server logs during model load wait ([#202](https://github.com/Meridiona/meridian/issues/202)) ([e369be7](https://github.com/Meridiona/meridian/commit/e369be7d8e6dc1432926b34c79f913c9162ead9c))

## [1.32.1](https://github.com/Meridiona/meridian/compare/v1.32.0...v1.32.1) (2026-06-09)


### Bug Fixes

* **installer:** install Python venv from PyPI at install time, drop pre-built tarball ([#201](https://github.com/Meridiona/meridian/issues/201)) ([9612cfb](https://github.com/Meridiona/meridian/commit/9612cfbd1ee963b28a28765f45d70c31ccbd2da6)), closes [#3](https://github.com/Meridiona/meridian/issues/3)

# [1.32.0](https://github.com/Meridiona/meridian/compare/v1.31.3...v1.32.0) (2026-06-08)


### Bug Fixes

* **coding-agent:** address PR [#193](https://github.com/Meridiona/meridian/issues/193) review findings ([333da23](https://github.com/Meridiona/meridian/commit/333da23e77b5d04bdc977517f1d719d0bb6b4ccb))
* **coding-agent:** read custom-title record type for Claude Code session names ([f1a8040](https://github.com/Meridiona/meridian/commit/f1a804078518083ff83bd275ce1679839d4fd0da))
* **coding-agent:** tick ceiling, ps probe timeout, restore today-only drain ([5641267](https://github.com/Meridiona/meridian/commit/56412673195e460cf7f1eee325c5de2908f945d9)), closes [#177](https://github.com/Meridiona/meridian/issues/177)
* **github:** address PR [#198](https://github.com/Meridiona/meridian/issues/198) review — non-Issue items + partial-fetch prune ([b5df2c0](https://github.com/Meridiona/meridian/commit/b5df2c082b73b94af024b84e3838185a06ceed1f))
* **install:** fix cursor-agent summariser check and messaging ([9901626](https://github.com/Meridiona/meridian/commit/9901626e3ad1511d053f60844b64359438a2844b))
* **setup:** no-PAT GitHub auth via gh CLI + correct read:project scope ([384c8de](https://github.com/Meridiona/meridian/commit/384c8de501a38f9bcd465a5ba7909cd2f0fc5443))
* **summariser:** dead-letter cap + two-day drain window; single-read titles ([23a608c](https://github.com/Meridiona/meridian/commit/23a608cbdf95b119dbbad23c6cdbb0325345b7f4))
* **ui:** resolve Turbopack NFT warning by lazily-initializing settings paths ([46b8305](https://github.com/Meridiona/meridian/commit/46b83057b95dd8f85ab5efe74d306a7275364146))


### Features

* **coding-agent:** populate window_titles from each agent's session name ([47f14db](https://github.com/Meridiona/meridian/commit/47f14dbafa66b06ff9f1ff30ebdd12bd14669332))
* **github:** replace Issues Search API with Projects v2 GraphQL ([35cbae9](https://github.com/Meridiona/meridian/commit/35cbae977890d610d65e54ca85ccb7b576edaf35))

## [1.31.3](https://github.com/Meridiona/meridian/compare/v1.31.2...v1.31.3) (2026-06-08)


### Bug Fixes

* **llm-selector:** pick largest fitting catalog model on low-RAM machines ([#197](https://github.com/Meridiona/meridian/issues/197)) ([7072106](https://github.com/Meridiona/meridian/commit/70721067a751738ade659a13ec40bf0df8ff5f89)), closes [#177](https://github.com/Meridiona/meridian/issues/177)
* **release:** always generate tray icons before build ([1ed15fc](https://github.com/Meridiona/meridian/commit/1ed15fce08363e2a341a10c16ac14aef2e768ccf))
* **release:** correct tray change-detection, sync Cargo.lock, cache tray npm ([9ec41e9](https://github.com/Meridiona/meridian/commit/9ec41e967d7bcb5377214b804139fd61d7c614aa))


### Performance Improvements

* **release:** optimize semantic-release build time by 1-2 minutes ([ad8c7a8](https://github.com/Meridiona/meridian/commit/ad8c7a8ce88ca5b7850b0e9d617ed587d2a207c8))

## [1.31.2](https://github.com/Meridiona/meridian/compare/v1.31.1...v1.31.2) (2026-06-08)


### Bug Fixes

* **tray:** support both dev and bundle binary paths in install script ([2be9204](https://github.com/Meridiona/meridian/commit/2be9204ccc075b3bc2d56d9ef421e275f740b28c))
* **tray:** use meridiona-mark logo instead of generic placeholder ([1a2fac4](https://github.com/Meridiona/meridian/commit/1a2fac41cc7435235fd860caa2c1e314bf4ea90f))

## [1.31.1](https://github.com/Meridiona/meridian/compare/v1.31.0...v1.31.1) (2026-06-08)


### Bug Fixes

* **install:** update tray binary path in launchd installer script ([671fd0a](https://github.com/Meridiona/meridian/commit/671fd0a45d1d5f3a76f572158944ceeeeef32081))

# [1.31.0](https://github.com/Meridiona/meridian/compare/v1.30.0...v1.31.0) (2026-06-08)


### Features

* **coding-agent:** session titles + custom-title fix + summariser reliability ([#193](https://github.com/Meridiona/meridian/issues/193)) ([0d4732f](https://github.com/Meridiona/meridian/commit/0d4732f1dc2a23367926134491bd01ed10c1e0ff)), closes [#177](https://github.com/Meridiona/meridian/issues/177)

# [1.30.0](https://github.com/Meridiona/meridian/compare/v1.29.0...v1.30.0) (2026-06-08)


### Bug Fixes

* **release:** update tray binary path to root target dir for workspace ([7608d4d](https://github.com/Meridiona/meridian/commit/7608d4d3d2dd566d5cc178e21e0a67dc97c8202b))
* **tray:** address reviewer comments from PR [#196](https://github.com/Meridiona/meridian/issues/196) ([4c79dcf](https://github.com/Meridiona/meridian/commit/4c79dcf07ec49409abd83c6b0f4834684876723b))
* **tray:** use MERIDIAN_UI_PORT env var instead of hardcoding 3939 ([35bdcdb](https://github.com/Meridiona/meridian/commit/35bdcdb016beb9f4ec5930a6d5d7cf3ea50b77e4))
* **ui:** add missing exec import in health route ([41965a2](https://github.com/Meridiona/meridian/commit/41965a25de27e7d0e416c43255521ccaf80993f3))


### Features

* **tray:** macOS menu bar app + daemon_running health signal ([f148b12](https://github.com/Meridiona/meridian/commit/f148b12012f1af40c4206454c3dab948ab037b49))
* **workspace:** add tray app to root Cargo workspace ([341d263](https://github.com/Meridiona/meridian/commit/341d26341ba24ea134e3a8c65b18c084b09e5bc7))

# [1.29.0](https://github.com/Meridiona/meridian/compare/v1.28.2...v1.29.0) (2026-06-08)


### Features

* **ui:** SSE health banner replaces 30s polling ([#195](https://github.com/Meridiona/meridian/issues/195)) ([4066ca9](https://github.com/Meridiona/meridian/commit/4066ca9ff6962a6fb3eefd92f91c35dc781a5e9d))

## [1.28.2](https://github.com/Meridiona/meridian/compare/v1.28.1...v1.28.2) (2026-06-08)


### Bug Fixes

* **ui:** replace blocking execSync in health route with async + cache ([12cb613](https://github.com/Meridiona/meridian/commit/12cb6134031ed5ed71e4fa23146b63a655a08cbf))

## [1.28.1](https://github.com/Meridiona/meridian/compare/v1.28.0...v1.28.1) (2026-06-08)


### Bug Fixes

* **release:** always ship ui.tar.gz so fresh installs work ([3759d49](https://github.com/Meridiona/meridian/commit/3759d494034397961b99b496f1775370e7d04cee))

# [1.28.0](https://github.com/Meridiona/meridian/compare/v1.27.6...v1.28.0) (2026-06-06)


### Features

* **coding-agent:** multi-agent conversation ingest + per-agent summarisation ([#184](https://github.com/Meridiona/meridian/issues/184)) ([15b000b](https://github.com/Meridiona/meridian/commit/15b000b2edc6bc3280ad217de491db8f152bfdb4))

## [1.27.6](https://github.com/Meridiona/meridian/compare/v1.27.5...v1.27.6) (2026-06-06)


### Bug Fixes

* **health:** suppress banners when doctor times out or has partial output ([354765e](https://github.com/Meridiona/meridian/commit/354765e5fbb2faa88dcbda2a96e4d878b89a91ba))

## [1.27.5](https://github.com/Meridiona/meridian/compare/v1.27.4...v1.27.5) (2026-06-06)


### Bug Fixes

* **health:** suppress DB-error banner when meridian binary cannot be found ([97622d7](https://github.com/Meridiona/meridian/commit/97622d7e35b0e51a84155d55cb8736046a831a55))

## [1.27.4](https://github.com/Meridiona/meridian/compare/v1.27.3...v1.27.4) (2026-06-06)


### Bug Fixes

* **health:** inject PATH for launchd and guard against command-not-found ([2dcc2b5](https://github.com/Meridiona/meridian/commit/2dcc2b5f7640fc316f01b3fab25fd26c4481a62f)), closes [#192](https://github.com/Meridiona/meridian/issues/192)
* **install:** production-harden installation scripts across all audit findings ([#192](https://github.com/Meridiona/meridian/issues/192)) ([d254ded](https://github.com/Meridiona/meridian/commit/d254ded473c1ee0bde11785e63b00f561404fc1d))

## [1.27.3](https://github.com/Meridiona/meridian/compare/v1.27.2...v1.27.3) (2026-06-06)


### Bug Fixes

* **install:** harden installer against partial-install and crash-loop failure modes ([#185](https://github.com/Meridiona/meridian/issues/185)) ([ee3159c](https://github.com/Meridiona/meridian/commit/ee3159c43d93b5ce3134caefefabef6bbc20943c))

## [1.27.2](https://github.com/Meridiona/meridian/compare/v1.27.1...v1.27.2) (2026-06-06)


### Bug Fixes

* **doctor:** detect bundle UI install in ui-built check ([#183](https://github.com/Meridiona/meridian/issues/183)) ([ebc8e33](https://github.com/Meridiona/meridian/commit/ebc8e337e70449c48cc73f511f441c5f27d0dfdb))

## [1.27.1](https://github.com/Meridiona/meridian/compare/v1.27.0...v1.27.1) (2026-06-06)


### Bug Fixes

* **health:** correct DB status string matching — stops false schema-mismatch banner ([#182](https://github.com/Meridiona/meridian/issues/182)) ([7229540](https://github.com/Meridiona/meridian/commit/72295401d3a4597b58612bde6d83325308a165f4))

# [1.27.0](https://github.com/Meridiona/meridian/compare/v1.26.2...v1.27.0) (2026-06-06)


### Features

* **cli:** add meridian update command for source checkouts ([#180](https://github.com/Meridiona/meridian/issues/180)) ([ebebd4a](https://github.com/Meridiona/meridian/commit/ebebd4a2dc9980a39975369d67790d7c5a602336))

## [1.26.2](https://github.com/Meridiona/meridian/compare/v1.26.1...v1.26.2) (2026-06-05)


### Bug Fixes

* add meridian migrate-db CLI command for npm users ([d3b11fe](https://github.com/Meridiona/meridian/commit/d3b11fe4f0a6a0531f655e9e53d61f1e9c540ab9))

## [1.26.1](https://github.com/Meridiona/meridian/compare/v1.26.0...v1.26.1) (2026-06-05)


### Bug Fixes

* handle database schema mismatches and improve health monitoring ([8b4ab51](https://github.com/Meridiona/meridian/commit/8b4ab512c638b4bdfccc2f8cc281b715b356e92d))

# [1.26.0](https://github.com/Meridiona/meridian/compare/v1.25.0...v1.26.0) (2026-06-05)


### Features

* **a11y-helper:** automate accessibility permission flow for Electron app capture ([46b0e97](https://github.com/Meridiona/meridian/commit/46b0e97869143f6d4b398c53acd49bbd9836b0aa))

# [1.25.0](https://github.com/Meridiona/meridian/compare/v1.24.6...v1.25.0) (2026-06-05)


### Bug Fixes

* **coding-agent:** ship session-summary skill file and install it during setup ([2c1161b](https://github.com/Meridiona/meridian/commit/2c1161be8f26a40086bf1d1f8d44773327289dda))
* **ui:** align OpenTelemetry packages after sdk-node 0.217 bump ([#179](https://github.com/Meridiona/meridian/issues/179)) ([96f4076](https://github.com/Meridiona/meridian/commit/96f4076afa72494de05d25b9ead1697552958f75)), closes [#173](https://github.com/Meridiona/meridian/issues/173)


### Features

* **capture:** make Electron apps visible to screenpipe (a11y-helper + doctor coverage) ([#178](https://github.com/Meridiona/meridian/issues/178)) ([9d722c9](https://github.com/Meridiona/meridian/commit/9d722c96c00e0c1525440eb587865b4bc26bf06f))
* **coding-agent:** add install-skill command and health check for session-summary ([#177](https://github.com/Meridiona/meridian/issues/177)) ([2f8cb1d](https://github.com/Meridiona/meridian/commit/2f8cb1d7fc2bab9575b900d90629d989c9928cf2))
* **install:** add --dev flag for development installs ([#175](https://github.com/Meridiona/meridian/issues/175)) ([a69ee75](https://github.com/Meridiona/meridian/commit/a69ee75418ab637b2a84e83d6b4aa534fc79d1a8))

## [1.24.6](https://github.com/Meridiona/meridian/compare/v1.24.5...v1.24.6) (2026-06-05)


### Bug Fixes

* **mlx-server:** use fresh OS thread for Apple FM to escape anyio event loop ([#174](https://github.com/Meridiona/meridian/issues/174)) ([7ea6519](https://github.com/Meridiona/meridian/commit/7ea6519517997afa040b61702e61924596612356))

## [1.24.5](https://github.com/Meridiona/meridian/compare/v1.24.4...v1.24.5) (2026-06-05)


### Bug Fixes

* **release:** revert parallel build step that regressed total time by 57s ([2d03164](https://github.com/Meridiona/meridian/commit/2d03164de35898692d245cf18c5cb1878d773286))

## [1.24.4](https://github.com/Meridiona/meridian/compare/v1.24.3...v1.24.4) (2026-06-05)


### Bug Fixes

* **mlx-server:** wrap Apple FM /classify in run_in_threadpool ([#172](https://github.com/Meridiona/meridian/issues/172)) ([9808d65](https://github.com/Meridiona/meridian/commit/9808d659adfcec9eea1dd3e73d936095af42a7d3))

## [1.24.3](https://github.com/Meridiona/meridian/compare/v1.24.2...v1.24.3) (2026-06-05)


### Bug Fixes

* **mlx-server:** compact Apple FM prompt to fix context-window overflow on /classify ([#170](https://github.com/Meridiona/meridian/issues/170)) ([0cf0a37](https://github.com/Meridiona/meridian/commit/0cf0a37cca2cbf4387b774064e39b78139782e98))

## [1.24.2](https://github.com/Meridiona/meridian/compare/v1.24.1...v1.24.2) (2026-06-05)


### Bug Fixes

* **mlx-server:** guard /classify against Apple FM sentinel + restart server on py-src change ([#169](https://github.com/Meridiona/meridian/issues/169)) ([bde48c3](https://github.com/Meridiona/meridian/commit/bde48c3d4adf5795f45e558fa1bacafbb7c780f8))


### Performance Improvements

* **release:** parallelize builds, cache Node 22 tarball, split named steps ([5299a05](https://github.com/Meridiona/meridian/commit/5299a05f679aac2c9076209069e8e1f1c388d00b))

## [1.24.1](https://github.com/Meridiona/meridian/compare/v1.24.0...v1.24.1) (2026-06-05)


### Bug Fixes

* **mlx-server:** route /v1/chat/completions + /summarise through Apple FM on 8 GB machines ([#168](https://github.com/Meridiona/meridian/issues/168)) ([99df470](https://github.com/Meridiona/meridian/commit/99df4704345694efa5d1713b7520f84655b6c6e0))

# [1.24.0](https://github.com/Meridiona/meridian/compare/v1.23.11...v1.24.0) (2026-06-05)


### Features

* **smoke:** add pipeline dry-run check to doctor and install ([e285d6e](https://github.com/Meridiona/meridian/commit/e285d6e664ef5fe8af894c13fed281df2cb97dcb))

## [1.23.11](https://github.com/Meridiona/meridian/compare/v1.23.10...v1.23.11) (2026-06-05)


### Bug Fixes

* **release:** ship Node runtime + Python venv off npm to fix 413 Payload Too Large ([1e6a96f](https://github.com/Meridiona/meridian/commit/1e6a96f2029527097805b1cd307181ca873027e9))

## [1.23.10](https://github.com/Meridiona/meridian/compare/v1.23.9...v1.23.10) (2026-06-05)


### Bug Fixes

* **install:** fall back to system node when bundled node-runtime absent ([#166](https://github.com/Meridiona/meridian/issues/166)) ([70a03fd](https://github.com/Meridiona/meridian/commit/70a03fd6c3fbcb66fec90e4b661629a66a53cdcc))
* **release:** add --prefix to better-sqlite3 Node 22 build so binary is findable ([#167](https://github.com/Meridiona/meridian/issues/167)) ([248fb32](https://github.com/Meridiona/meridian/commit/248fb32b5966f947325b5d762c658755a619f052))
* **release:** compile better-sqlite3 with node-gyp directly, skip npm lifecycle ([585ce6a](https://github.com/Meridiona/meridian/commit/585ce6acec4422258a269311118b0ac2ee232b6a))
* **release:** download better-sqlite3 prebuilt directly, skip compilation ([63ffab4](https://github.com/Meridiona/meridian/commit/63ffab4339abc0f26d1443b0c30c60d0a0fc1217))
* **release:** prepend Node 22 bin to PATH so prebuild-install uses ABI 127 ([f2a78bd](https://github.com/Meridiona/meridian/commit/f2a78bdc05a071c1d4434f925def8395261d39d6))
* **release:** require .node binary directly in ABI checks, not the package ([4b36173](https://github.com/Meridiona/meridian/commit/4b36173ef4ea2c07712cd74c03c9da90420366f1))
* **release:** unset npm_config_nodedir so node-gyp uses Node 22 headers ([af324d1](https://github.com/Meridiona/meridian/commit/af324d1bfad9c60dd7f90685961d994aa86ca102))
* **release:** use absolute path for better-sqlite3 require() load check ([6dbf299](https://github.com/Meridiona/meridian/commit/6dbf2994d9df1aff519c1a4d6219861a68af8998))
* **ui:** bundle Node 22 runtime so better-sqlite3 ABI never mismatches ([#165](https://github.com/Meridiona/meridian/issues/165)) ([a9fa2f1](https://github.com/Meridiona/meridian/commit/a9fa2f18ba5cd37f3d8cf3776cdebdbc9f6f0314))

## [1.23.9](https://github.com/Meridiona/meridian/compare/v1.23.8...v1.23.9) (2026-06-05)


### Performance Improvements

* **release:** conditional UI tarball — skip ~10 MB download when dashboard unchanged ([#164](https://github.com/Meridiona/meridian/issues/164)) ([f241f95](https://github.com/Meridiona/meridian/commit/f241f959cc0e1d807126d1dd3dcd571d69b7a62a))

## [1.23.8](https://github.com/Meridiona/meridian/compare/v1.23.7...v1.23.8) (2026-06-05)


### Bug Fixes

* **ui:** self-heal better-sqlite3 ABI mismatch at daemon startup ([#163](https://github.com/Meridiona/meridian/issues/163)) ([f0e0ddf](https://github.com/Meridiona/meridian/commit/f0e0ddf8bd7480e64709f1eaddbee1a0d05bde05))

## [1.23.7](https://github.com/Meridiona/meridian/compare/v1.23.6...v1.23.7) (2026-06-04)


### Bug Fixes

* **intelligence:** always ship venv tarball on macOS 26+ to preserve apple-fm-sdk ([#162](https://github.com/Meridiona/meridian/issues/162)) ([724e1ed](https://github.com/Meridiona/meridian/commit/724e1eddff2844870053379c2cc62623f7423bca))

## [1.23.6](https://github.com/Meridiona/meridian/compare/v1.23.5...v1.23.6) (2026-06-04)


### Bug Fixes

* **ui:** rebuild better-sqlite3 when Node ABI mismatches CI build ([#161](https://github.com/Meridiona/meridian/issues/161)) ([9471bd9](https://github.com/Meridiona/meridian/commit/9471bd94df08fda13fd6a26102d4a17b767400ee))

## [1.23.5](https://github.com/Meridiona/meridian/compare/v1.23.4...v1.23.5) (2026-06-04)


### Bug Fixes

* **intelligence:** enforce Python 3.11 venv when extracting pre-built tarball ([#160](https://github.com/Meridiona/meridian/issues/160)) ([a22881d](https://github.com/Meridiona/meridian/commit/a22881d098122d5a3ae7acc50c4fbe0c7a0e5240))

## [1.23.4](https://github.com/Meridiona/meridian/compare/v1.23.3...v1.23.4) (2026-06-04)


### Bug Fixes

* **intelligence:** use uv pip instead of pip binary for apple-fm-sdk install ([#159](https://github.com/Meridiona/meridian/issues/159)) ([bfe4a88](https://github.com/Meridiona/meridian/commit/bfe4a8880917df7fff841d7761bafe4daa6f2f16))

## [1.23.3](https://github.com/Meridiona/meridian/compare/v1.23.2...v1.23.3) (2026-06-04)


### Bug Fixes

* **intelligence:** install apple-fm-sdk in npm bundle path on macOS 26+ ([#157](https://github.com/Meridiona/meridian/issues/157)) ([096cdec](https://github.com/Meridiona/meridian/commit/096cdec8999c2cbddddf45ecc495972614f5988f))

## [1.23.2](https://github.com/Meridiona/meridian/compare/v1.23.1...v1.23.2) (2026-06-04)


### Bug Fixes

* **intelligence:** gate Apple Intelligence on actual availability, not just macOS version ([#155](https://github.com/Meridiona/meridian/issues/155)) ([5de2787](https://github.com/Meridiona/meridian/commit/5de278728772dab1acf53dc7cc70cc7d679a33d7))
* **intelligence:** install apple-fm-sdk, check Xcode, cap prompt for Apple FM ([#156](https://github.com/Meridiona/meridian/issues/156)) ([02632b3](https://github.com/Meridiona/meridian/commit/02632b398e5ffc5e91a0269574129d9437fab676))

## [1.23.1](https://github.com/Meridiona/meridian/compare/v1.23.0...v1.23.1) (2026-06-03)


### Bug Fixes

* **hooks:** fix uninstall-claude-hook.sh idempotency detection ([#152](https://github.com/Meridiona/meridian/issues/152)) ([45a16e8](https://github.com/Meridiona/meridian/commit/45a16e833e1ecbb12e42291d9a69d6591498dff7))

# [1.23.0](https://github.com/Meridiona/meridian/compare/v1.22.1...v1.23.0) (2026-06-03)


### Features

* **intelligence:** route 8 GB machines to Apple Intelligence for MLX classify ([#151](https://github.com/Meridiona/meridian/issues/151)) ([7880bc0](https://github.com/Meridiona/meridian/commit/7880bc0e1dc20cd58f0f34c157c7b7f09e3e90d7))

## [1.22.1](https://github.com/Meridiona/meridian/compare/v1.22.0...v1.22.1) (2026-06-03)


### Bug Fixes

* **hooks:** fix claude hook idempotency and add coding-agent passthrough ([#150](https://github.com/Meridiona/meridian/issues/150)) ([911b743](https://github.com/Meridiona/meridian/commit/911b743283683d1df92475e9fe2d67ddd9ae6a57))
* **setup:** ship agno in the venv so /synthesise_worklog stops 500ing ([#149](https://github.com/Meridiona/meridian/issues/149)) ([bfef781](https://github.com/Meridiona/meridian/commit/bfef781cfcbb8f8d96ab3e1d7c8ac6f5d96c6dcd))

# [1.22.0](https://github.com/Meridiona/meridian/compare/v1.21.0...v1.22.0) (2026-06-03)


### Features

* **intelligence:** gate task-linking + worklog on a working pipeline ([#147](https://github.com/Meridiona/meridian/issues/147)) ([b99c0dc](https://github.com/Meridiona/meridian/commit/b99c0dcd317af2c09894eaddf1090a05830572b5))

# [1.21.0](https://github.com/Meridiona/meridian/compare/v1.20.1...v1.21.0) (2026-06-03)


### Features

* **intelligence:** gate task-linking + worklog on a working pipeline ([#146](https://github.com/Meridiona/meridian/issues/146)) ([d2d84f8](https://github.com/Meridiona/meridian/commit/d2d84f86fce93ec8e3b9870e570d09c314f367b2))

## [1.20.1](https://github.com/Meridiona/meridian/compare/v1.20.0...v1.20.1) (2026-06-03)


### Bug Fixes

* **install:** detect npm launcher at the real prefix, not just /usr/local ([#144](https://github.com/Meridiona/meridian/issues/144)) ([e1cfb3b](https://github.com/Meridiona/meridian/commit/e1cfb3bdea81171918f2fad792b592e9e5c2e90f))

# [1.20.0](https://github.com/Meridiona/meridian/compare/v1.19.0...v1.20.0) (2026-06-03)


### Features

* **ui:** flag unconnected trackers in the Tasks empty state ([#143](https://github.com/Meridiona/meridian/issues/143)) ([df4b836](https://github.com/Meridiona/meridian/commit/df4b8363200662858f092732a2690fb522be2339))

# [1.19.0](https://github.com/Meridiona/meridian/compare/v1.18.0...v1.19.0) (2026-06-03)


### Bug Fixes

* **cli:** correct misleading update time message; add version command ([70c742a](https://github.com/Meridiona/meridian/commit/70c742a49b8be4ac6cce6db1e98c8108e24f09b5))


### Features

* **cli:** add meridian version / --version / -v command ([a712fb3](https://github.com/Meridiona/meridian/commit/a712fb33620e5bee99c2828fc2e16acbf37cb85f))
* **release:** only ship venv tarball when uv.lock changes ([#142](https://github.com/Meridiona/meridian/issues/142)) ([e83203b](https://github.com/Meridiona/meridian/commit/e83203be4da6b5b99149c1f817656b8f95fe18ea))

# [1.18.0](https://github.com/Meridiona/meridian/compare/v1.17.1...v1.18.0) (2026-06-02)


### Bug Fixes

* **ci:** pin Python 3.11 in venv build to match user machines ([#141](https://github.com/Meridiona/meridian/issues/141)) ([130c0e6](https://github.com/Meridiona/meridian/commit/130c0e689b844c2591d98f669e4292b3f413b730))


### Features

* **ci:** cache npm, uv packages and Next.js build in release workflow ([#140](https://github.com/Meridiona/meridian/issues/140)) ([52d6f21](https://github.com/Meridiona/meridian/commit/52d6f211a8051b4bc31ba9343a2ca47498d64d65))

## [1.17.1](https://github.com/Meridiona/meridian/compare/v1.17.0...v1.17.1) (2026-06-02)


### Bug Fixes

* **update:** re-exec new launcher to self-heal old launchers in one step ([#138](https://github.com/Meridiona/meridian/issues/138)) ([e72f3fc](https://github.com/Meridiona/meridian/commit/e72f3fc84a4fe6aea86c106b4a92ca422f9ac509))

# [1.17.0](https://github.com/Meridiona/meridian/compare/v1.16.0...v1.17.0) (2026-06-02)


### Bug Fixes

* **update:** explicitly install darwin-arm64 bundle in meridian update ([#137](https://github.com/Meridiona/meridian/issues/137)) ([4cdf8b4](https://github.com/Meridiona/meridian/commit/4cdf8b4b0a2bf695cce9d27b1727990b511c0086))


### Features

* **classifier:** compact Apple Intelligence system prompt (SKILL.applefm.md) ([#133](https://github.com/Meridiona/meridian/issues/133)) ([0c77f17](https://github.com/Meridiona/meridian/commit/0c77f176921ba0c8e02388d9e5187205475f3236)), closes [114/#116](https://github.com/Meridiona/meridian/issues/116)

# [1.16.0](https://github.com/Meridiona/meridian/compare/v1.15.1...v1.16.0) (2026-06-02)


### Features

* **update:** show download size + elapsed time during meridian update ([#136](https://github.com/Meridiona/meridian/issues/136)) ([498660b](https://github.com/Meridiona/meridian/commit/498660bfbb7bf22bf0a42295b9520fc60188072c))

## [1.15.1](https://github.com/Meridiona/meridian/compare/v1.15.0...v1.15.1) (2026-06-02)


### Bug Fixes

* **setup:** add services-venv.tar.gz to npm files; fix grep set -e exit ([#135](https://github.com/Meridiona/meridian/issues/135)) ([7e5faf2](https://github.com/Meridiona/meridian/commit/7e5faf264508eb0bd2140481caf566fae09842a1))

# [1.15.0](https://github.com/Meridiona/meridian/compare/v1.14.2...v1.15.0) (2026-06-02)


### Bug Fixes

* **ci:** use brew install uv instead of pip3 in package-release.sh ([#134](https://github.com/Meridiona/meridian/issues/134)) ([c6deb19](https://github.com/Meridiona/meridian/commit/c6deb19f1633c6685b24d37b4d74ff439e3ae3a1))


### Features

* **setup:** pre-build Python venv in CI, extract at install (~5s vs 40s) ([#132](https://github.com/Meridiona/meridian/issues/132)) ([5e3e0a8](https://github.com/Meridiona/meridian/commit/5e3e0a8c5a0488cdf9ca9cbf88d754a9465a2ae0))

## [1.14.2](https://github.com/Meridiona/meridian/compare/v1.14.1...v1.14.2) (2026-06-02)


### Bug Fixes

* **mlx-server:** parse uv pyvenv.cfg format for BASE_PYTHON resolution ([#131](https://github.com/Meridiona/meridian/issues/131)) ([802c429](https://github.com/Meridiona/meridian/commit/802c42951e2aecd32985b3e4056651e55dd64718))

## [1.14.1](https://github.com/Meridiona/meridian/compare/v1.14.0...v1.14.1) (2026-06-02)


### Bug Fixes

* **bootstrap:** use prefix ownership to detect root-owned npm prefix ([#130](https://github.com/Meridiona/meridian/issues/130)) ([53715f9](https://github.com/Meridiona/meridian/commit/53715f95ca055255bd0428188635a5edeab9548b))

# [1.14.0](https://github.com/Meridiona/meridian/compare/v1.13.0...v1.14.0) (2026-06-02)


### Features

* **setup:** add bootstrap.sh — fixes npm prefix so no sudo is needed ([#128](https://github.com/Meridiona/meridian/issues/128)) ([d41060e](https://github.com/Meridiona/meridian/commit/d41060e48d07fa63181f2156af0f82c801ddf522))

# [1.13.0](https://github.com/Meridiona/meridian/compare/v1.12.0...v1.13.0) (2026-06-02)


### Features

* **setup:** adopt uv for reproducible Python venv (replaces pip lockfile) ([#127](https://github.com/Meridiona/meridian/issues/127)) ([228991c](https://github.com/Meridiona/meridian/commit/228991c6e6a17f2e14f4a658fac62b563e120c94)), closes [#126](https://github.com/Meridiona/meridian/issues/126)

# [1.12.0](https://github.com/Meridiona/meridian/compare/v1.11.0...v1.12.0) (2026-06-02)


### Bug Fixes

* **setup:** pin the MLX Python deps with a lockfile for reproducible installs ([#126](https://github.com/Meridiona/meridian/issues/126)) ([73d3f6a](https://github.com/Meridiona/meridian/commit/73d3f6a0161e9217d0cdc32190b288ecac5fdfd8))


### Features

* **ui:** in-dashboard update notification + one-click update (Fix B) ([#125](https://github.com/Meridiona/meridian/issues/125)) ([2cf6343](https://github.com/Meridiona/meridian/commit/2cf6343a918dfc6ef84776d3e1cd3ba356308473))

# [1.11.0](https://github.com/Meridiona/meridian/compare/v1.10.1...v1.11.0) (2026-06-02)


### Features

* **update:** preserve the Python venv across `meridian update` (seconds, not minutes) ([#124](https://github.com/Meridiona/meridian/issues/124)) ([58a31d7](https://github.com/Meridiona/meridian/commit/58a31d7fa9f402bc30f54f629146fc6f90679dbd))

## [1.10.1](https://github.com/Meridiona/meridian/compare/v1.10.0...v1.10.1) (2026-06-02)


### Bug Fixes

* **screenpipe:** launch the real Mach-O so macOS grants attach to screenpipe, not node ([#123](https://github.com/Meridiona/meridian/issues/123)) ([6afa180](https://github.com/Meridiona/meridian/commit/6afa180cff5f0d5089f830af5960fbf803ce8731))

# [1.10.0](https://github.com/Meridiona/meridian/compare/v1.9.2...v1.10.0) (2026-06-02)


### Features

* **ui:** ship Turbopack standalone via symlink-preserving tarball (keep Turbopack in prod) ([#122](https://github.com/Meridiona/meridian/issues/122)) ([5c7931c](https://github.com/Meridiona/meridian/commit/5c7931c46b468f0e75611b358acad5462f0df726)), closes [vercel/next.js#87737](https://github.com/vercel/next.js/issues/87737) [#93849](https://github.com/Meridiona/meridian/issues/93849) [#121](https://github.com/Meridiona/meridian/issues/121)

## [1.9.2](https://github.com/Meridiona/meridian/compare/v1.9.1...v1.9.2) (2026-06-02)


### Bug Fixes

* **npm:** stop `sudo meridian update` from breaking launchd; elevate only npm ([#120](https://github.com/Meridiona/meridian/issues/120)) ([eb4377b](https://github.com/Meridiona/meridian/commit/eb4377b7f421b39424a526c64615b8cbe958e290))

## [1.9.1](https://github.com/Meridiona/meridian/compare/v1.9.0...v1.9.1) (2026-06-02)


### Bug Fixes

* **setup,doctor:** harden a11y-tree capture for VS Code-family editors ([#117](https://github.com/Meridiona/meridian/issues/117)) ([c47cf55](https://github.com/Meridiona/meridian/commit/c47cf55b0bedd9ec4eec2f56f502e33b2d75971c))
* **ui:** build standalone with webpack — Turbopack ships an unresolvable better-sqlite3 external ([#118](https://github.com/Meridiona/meridian/issues/118)) ([29de7f0](https://github.com/Meridiona/meridian/commit/29de7f0bc694367325170da4b86fbfd6d8b67b89)), closes [vercel/next.js#88844](https://github.com/vercel/next.js/issues/88844) [#87737](https://github.com/Meridiona/meridian/issues/87737)

# [1.9.0](https://github.com/Meridiona/meridian/compare/v1.8.2...v1.9.0) (2026-06-02)


### Bug Fixes

* **llm-selector:** correct Apple FoundationModels SDK API usage ([#114](https://github.com/Meridiona/meridian/issues/114)) ([70f71bb](https://github.com/Meridiona/meridian/commit/70f71bb13e917e65da5f394394d23560220d1d5a))


### Features

* **agents:** dynamically select the MLX server model via llm_selector ([#112](https://github.com/Meridiona/meridian/issues/112)) ([d77257e](https://github.com/Meridiona/meridian/commit/d77257ea0ac88aa3427dff3bf0d2c920761165e0))

## [1.8.2](https://github.com/Meridiona/meridian/compare/v1.8.1...v1.8.2) (2026-06-02)


### Bug Fixes

* **ui:** don't load OpenTelemetry at boot — fixes dashboard 500 in standalone ([#115](https://github.com/Meridiona/meridian/issues/115)) ([a8a7ed1](https://github.com/Meridiona/meridian/commit/a8a7ed1c944cb8b5417966e489469372f77bdb78))

## [1.8.1](https://github.com/Meridiona/meridian/compare/v1.8.0...v1.8.1) (2026-06-02)


### Bug Fixes

* **npm:** stop bundle setup shadowing the npm launcher's meridian CLI ([#113](https://github.com/Meridiona/meridian/issues/113)) ([8cb8a63](https://github.com/Meridiona/meridian/commit/8cb8a634ddb06e06703a8aa7006b910b8629b2ac))

# [1.8.0](https://github.com/Meridiona/meridian/compare/v1.7.0...v1.8.0) (2026-06-02)


### Bug Fixes

* **deps:** patch npm security advisories (next, postcss, protobufjs, qs) ([#111](https://github.com/Meridiona/meridian/issues/111)) ([9350199](https://github.com/Meridiona/meridian/commit/9350199f78b8fea1657eebe3eeaf360324d8c8e9))


### Features

* **pm-worklog:** draft worklogs on classify event, fix backfill timer starvation ([1f52958](https://github.com/Meridiona/meridian/commit/1f52958fdd4e77167c1a3d09556eeaabb22868e3))

# [1.7.0](https://github.com/Meridiona/meridian/compare/v1.6.0...v1.7.0) (2026-06-02)


### Features

* **health:** comprehensive multi-daemon meridian doctor (+ settings-path fix) ([#108](https://github.com/Meridiona/meridian/issues/108)) ([5186b29](https://github.com/Meridiona/meridian/commit/5186b29b1dcaf18f2e05f550aa70b99484b088cd))
* **worklog:** capture review actions + reject attribution for eval feedback ([#107](https://github.com/Meridiona/meridian/issues/107)) ([aec928a](https://github.com/Meridiona/meridian/commit/aec928aba7ae127a64615930efb3012945336697))

# [1.6.0](https://github.com/Meridiona/meridian/compare/v1.5.0...v1.6.0) (2026-06-02)


### Bug Fixes

* **classifier:** drop category-input reference from output schema ([e0c66b0](https://github.com/Meridiona/meridian/commit/e0c66b055d7f17f4a2254cb458bb18f5c4f4ed31))
* **classifier:** stop feeding category into task classification ([f0db2bc](https://github.com/Meridiona/meridian/commit/f0db2bc658c2259f24dd2d0e353a8b85f07a5624))
* **evals:** make the LLM judge optional; add classify_session debug tool ([09393b6](https://github.com/Meridiona/meridian/commit/09393b61ad44a2dca9b12400053281b384628831))


### Features

* **classifier:** raise SESSION_TEXT_CAP from 2500 to 10000 ([f248384](https://github.com/Meridiona/meridian/commit/f2483848617fc9ca328fad7a69041cb9ed10e633))

# [1.5.0](https://github.com/Meridiona/meridian/compare/v1.4.5...v1.5.0) (2026-06-02)


### Features

* **worklog:** add Linear and GitHub worklog providers alongside Jira ([#102](https://github.com/Meridiona/meridian/issues/102)) ([fb030e2](https://github.com/Meridiona/meridian/commit/fb030e2e9fc475f2dd6495931fbad3cacfa84321))

## [1.4.5](https://github.com/Meridiona/meridian/compare/v1.4.4...v1.4.5) (2026-06-02)


### Bug Fixes

* **ui:** per-task time = your time + autonomous agent, with insight labels ([#101](https://github.com/Meridiona/meridian/issues/101)) ([c2111d0](https://github.com/Meridiona/meridian/commit/c2111d0ccb671ac95c92cf19b79f1ecc867bdcfc))

## [1.4.4](https://github.com/Meridiona/meridian/compare/v1.4.3...v1.4.4) (2026-06-01)


### Bug Fixes

* **ui:** per-task time = union of total time, consistent across views ([#100](https://github.com/Meridiona/meridian/issues/100)) ([4508c13](https://github.com/Meridiona/meridian/commit/4508c1321720775d50062f3f01c7cabc87b42b91))

## [1.4.3](https://github.com/Meridiona/meridian/compare/v1.4.2...v1.4.3) (2026-06-01)


### Bug Fixes

* **daemon:** add PATH to launchd plist so claude/codex resolve ([#97](https://github.com/Meridiona/meridian/issues/97)) ([e8abab2](https://github.com/Meridiona/meridian/commit/e8abab2a0ce5e847f4b7d39e389bec87d3ee2528))
* **summariser:** log primary-engine failure on MLX fallback ([#98](https://github.com/Meridiona/meridian/issues/98)) ([0827f01](https://github.com/Meridiona/meridian/commit/0827f01b24dfa8f506602bcb7c2813ac96316eed))
* **task-linker:** classify coding-agent rows one per MLX call ([#96](https://github.com/Meridiona/meridian/issues/96)) ([69f9e65](https://github.com/Meridiona/meridian/commit/69f9e65febd89efddd7156fb56798ee608018f72))

## [1.4.2](https://github.com/Meridiona/meridian/compare/v1.4.1...v1.4.2) (2026-06-01)


### Bug Fixes

* **ui:** cache the read-only sqlite handle in production ([#93](https://github.com/Meridiona/meridian/issues/93)) ([a312bcc](https://github.com/Meridiona/meridian/commit/a312bccdfb8a98043743b86412caa245462e8ec2))
* **ui:** pin turbopack workspace root to ui/ ([#92](https://github.com/Meridiona/meridian/issues/92)) ([c1e2bb7](https://github.com/Meridiona/meridian/commit/c1e2bb7bab936b3b22da7aca201f070c9256c7d8))

## [1.4.1](https://github.com/Meridiona/meridian/compare/v1.4.0...v1.4.1) (2026-06-01)


### Bug Fixes

* **cli:** meridian dev — hot-reload UI + reliable service bring-up ([#90](https://github.com/Meridiona/meridian/issues/90)) ([153474a](https://github.com/Meridiona/meridian/commit/153474a89dfe93590f49e60882bad1c02baed2ea))
* **ui:** split untracked time out of the overhead bucket ([#91](https://github.com/Meridiona/meridian/issues/91)) ([8ea9d3a](https://github.com/Meridiona/meridian/commit/8ea9d3aa8da79564ca496a0d5bf13f70ce3be87c))

# [1.4.0](https://github.com/Meridiona/meridian/compare/v1.3.0...v1.4.0) (2026-06-01)


### Features

* **cli:** meridian dev commands + dev-workflow docs ([#89](https://github.com/Meridiona/meridian/issues/89)) ([5a09dba](https://github.com/Meridiona/meridian/commit/5a09dba546a877f8192aa769a7b6537d88675752))

# [1.3.0](https://github.com/Meridiona/meridian/compare/v1.2.2...v1.3.0) (2026-06-01)


### Features

* **pm-worklog:** add approval state + migration for human-in-the-loop posting ([1c84ddc](https://github.com/Meridiona/meridian/commit/1c84ddcfb41a140f78c1baa2677d4a6cb3d419a1))
* **pm-worklog:** draft-only driver + approved-poster sweep ([d0094b2](https://github.com/Meridiona/meridian/commit/d0094b2921e14de786242f5b82510124c01df568))
* **ui:** insight-first Today view — presence + agent overlay, progressive disclosure ([92ca7fb](https://github.com/Meridiona/meridian/commit/92ca7fb0cab66fbe90c5ea514900585aa150672e))
* **ui:** wire Worklogs into the dashboard nav ([fcca5fe](https://github.com/Meridiona/meridian/commit/fcca5febce5288be9acdad4f948b2de43da0a234))
* **ui:** worklog API — list + edit/approve/reject (DB writes only) ([cdd1af4](https://github.com/Meridiona/meridian/commit/cdd1af48407a9e56296901cec8f57eb879239e04))
* **ui:** Worklogs review view — approve before it posts ([cc08a3e](https://github.com/Meridiona/meridian/commit/cc08a3e559304a8913028ab22eab4bb17dbc4565))

## [1.2.2](https://github.com/Meridiona/meridian/compare/v1.2.1...v1.2.2) (2026-06-01)


### Bug Fixes

* **ui:** union overlapping activity for Focus instead of summing streams ([e620f45](https://github.com/Meridiona/meridian/commit/e620f45ca4c16beb6fb26f47993e31ce213a5cdd))

## [1.2.1](https://github.com/Meridiona/meridian/compare/v1.2.0...v1.2.1) (2026-06-01)


### Bug Fixes

* **release:** migrate npm publish to OIDC trusted publishing (drop NPM_TOKEN) ([#79](https://github.com/Meridiona/meridian/issues/79)) ([50f6066](https://github.com/Meridiona/meridian/commit/50f606640231e7bd2852993b28fed14b378ee76c))

# [1.2.0](https://github.com/Meridiona/meridian/compare/v1.1.0...v1.2.0) (2026-06-01)


### Features

* **pm-worklog:** add worklog-status CLI report command ([#78](https://github.com/Meridiona/meridian/issues/78)) ([6cc120c](https://github.com/Meridiona/meridian/commit/6cc120c7f7057eb76f76ff8585724804bc0ae67b))

# [1.1.0](https://github.com/Meridiona/meridian/compare/v1.0.0...v1.1.0) (2026-06-01)


### Features

* **install:** dashboard on port 3939 (overridable) + disable screenpipe audio ([#77](https://github.com/Meridiona/meridian/issues/77)) ([4182b93](https://github.com/Meridiona/meridian/commit/4182b931611e8428b6705e011ee0955e8edcdd63))

# 1.0.0 (2026-06-01)


* feat(migrations)!: rename 004→005 and rework agent schema ([c390a91](https://github.com/Meridiona/meridian/commit/c390a91cc5fe01d06847e81e521d493c9bc70380))
* feat(migrations)!: rename 004→005 and rework agent schema ([a405be6](https://github.com/Meridiona/meridian/commit/a405be67c99b985c4a836a2ace3857b1b0c0dff9))


### Bug Fixes

* **agents:** address all PR review changes for jira-updater and meridian-mcp ([70dfb39](https://github.com/Meridiona/meridian/commit/70dfb3959da87270dbe34fec46eb43c2b7a309f1))
* **agents:** address PR [#3](https://github.com/Meridiona/meridian/issues/3) review — defer hermes setup, fix header + docstring + cursor warning ([70e6d07](https://github.com/Meridiona/meridian/commit/70e6d070732c85de3c95fdaeef9fe1d748f08083))
* **agents:** resolve critical issues in jira-updater found in code review ([2613ca6](https://github.com/Meridiona/meridian/commit/2613ca6e2f6bc48c7e00d1429479ba5f7284f4ce))
* **agents:** validate MERIDIAN_DB env var at config load to prevent prompt injection, shell injection, and path traversal ([40a7147](https://github.com/Meridiona/meridian/commit/40a7147b5cb4b4ea2106b88620940ba037723844))
* **ax-sidecar:** harden against missing deps, transient failures, and idle states ([b1e3867](https://github.com/Meridiona/meridian/commit/b1e3867017f7674adad29cebf4715758938e3407))
* **ax-sidecar:** use OS process tree for host app detection + scan subagents/ ([1b25123](https://github.com/Meridiona/meridian/commit/1b251231299273e0456bb7c5d04cab89d93c5f92))
* **bootstrap:** update synthesizer references to tagger, fix pm_tasks schema ([578fc8d](https://github.com/Meridiona/meridian/commit/578fc8de42a1510381f64694f1dc144a356a15d2))
* **categorizer:** correct misclassifications found in real session smoke test ([c1bd8f4](https://github.com/Meridiona/meridian/commit/c1bd8f4b0e716d056414d1baaaf13f5d93258fdf))
* **categorizer:** fix false-positive DevOps from YouTube/README a11y elements ([c134d70](https://github.com/Meridiona/meridian/commit/c134d7048fb13765d89248b770999322c55d1525))
* **classifier:** drop the 300-char cap on the reasoning field ([7823758](https://github.com/Meridiona/meridian/commit/782375854258e0b7087cf864257ae98152c3c392))
* **classifier:** enforce JSON output and disable Qwen3 thinking mode ([01b564b](https://github.com/Meridiona/meridian/commit/01b564b5c2e1760e728855ca3883d590a219408d))
* **classifier:** robust response extraction and JSON sanitization ([4fefe19](https://github.com/Meridiona/meridian/commit/4fefe190823d2e581021b44fc2489784328f9f27))
* **classifier:** stop outlines truncating task reasoning at 300 chars ([8dbaff7](https://github.com/Meridiona/meridian/commit/8dbaff78edeb493479eae0369456420511ae6edf))
* **cli:** add mlx-server to managed labels; kill orphaned mlx_lm.server on stop ([7139796](https://github.com/Meridiona/meridian/commit/71397967f93cfb2ad739035b9d97bb632d902693))
* **config:** disable jira updater by default, skip gracefully if module missing ([7a94a4a](https://github.com/Meridiona/meridian/commit/7a94a4a32350f1f29dcf3cc78a3e342206cf78d8))
* **css:** use direct path for tailwindcss import ([e81c7cc](https://github.com/Meridiona/meridian/commit/e81c7cc8ef58fa81a0e2eda5731f8b79a5649971))
* **db:** improve window title normalization for non-browser apps ([aa7f02a](https://github.com/Meridiona/meridian/commit/aa7f02a879a84ff50a63835808af1279a65fdfba))
* **db:** remove LIMIT 10 from get_element_samples and add TRIM dedup ([36ca205](https://github.com/Meridiona/meridian/commit/36ca205c6042bb2d530a6517a31e046b39514ef6))
* **donut:** eliminate Recharts SSR hydration mismatch ([1e269f2](https://github.com/Meridiona/meridian/commit/1e269f241b7c71c343cf32f71ce98eee320d3474))
* **etl:** close block on gap so session duration_s excludes gap time ([66e81ce](https://github.com/Meridiona/meridian/commit/66e81ce722e57dc1c1efdb5fb128b1522ee501da))
* **etl:** deduplicate AXTextArea — keep only last snapshot per session ([d653c6c](https://github.com/Meridiona/meridian/commit/d653c6c6978714b209de938af1d2a0bea6d849c3))
* **etl:** detect sleep gaps that span ETL run boundaries ([a8f2280](https://github.com/Meridiona/meridian/commit/a8f22805abe9fdc059c55dab91a6598ab67f66e9))
* **etl:** fix three gap detection bugs ([a63d0e6](https://github.com/Meridiona/meridian/commit/a63d0e62ca286ed365c675d7af6f3345f89a40f7))
* **etl:** merge reclassified Terminal sessions correctly ([7af11f8](https://github.com/Meridiona/meridian/commit/7af11f84bdfe2a2b44420f5b5ad53a127d3bdca2))
* **etl:** merge reclassified Terminal sessions correctly ([f03b61b](https://github.com/Meridiona/meridian/commit/f03b61ba2e583112f5ab8ce66d96d2e782412fac))
* **etl:** recover inter-frame gap in session duration (extend Option D) ([06bc9ab](https://github.com/Meridiona/meridian/commit/06bc9ab50f26af0db18d56027965c9c94c1a1ab0))
* **etl:** refactor block args into BlockBounds to satisfy clippy ([1a5ef2b](https://github.com/Meridiona/meridian/commit/1a5ef2b1115ff6e87c7559d57af295b5cadc8c2a))
* **etl:** remove LIMIT cap on ocr_samples and elements_samples extraction ([b716085](https://github.com/Meridiona/meridian/commit/b716085627f379fbc9a1f5bf3bac8e1bd90413cd))
* **etl:** resolve 0s duration on single-frame sessions (Option D) ([317ceb2](https://github.com/Meridiona/meridian/commit/317ceb211333d9c1a4b73f48e33c285da6498194))
* **etl:** restore LIMIT 10 on elements_samples extraction ([1032b65](https://github.com/Meridiona/meridian/commit/1032b65d91a282f16f694ebf88dd5328c047584d))
* **evals:** correct model label phi-4-4bit → Qwen3.5-9B-OptiQ-4bit + TODO for /info endpoint ([925b1e8](https://github.com/Meridiona/meridian/commit/925b1e896305208efc1ae8ed20bbcd78cffa300e)), closes [#1](https://github.com/Meridiona/meridian/issues/1) [#1](https://github.com/Meridiona/meridian/issues/1)
* **evals:** force-flush root eval.run span before obs_shutdown ([25af90d](https://github.com/Meridiona/meridian/commit/25af90d662157ec28feab9533af753d5a3d90674))
* **evals:** wire Dev B candidates into render_seeds.py — normalise id/task_key ([6a39692](https://github.com/Meridiona/meridian/commit/6a39692ee5822a5d064b125a86e1ba17ab35cd0a))
* **fmt:** apply cargo fmt to intelligence modules ([ca49a35](https://github.com/Meridiona/meridian/commit/ca49a357ea6fdf57856d215a0f084d2965e94638))
* **hooks:** block edits to committed migration files in pre-commit ([4555892](https://github.com/Meridiona/meridian/commit/455589217af772ef62e0498949d4533e9328cdd7))
* **install:** correct permissions walkthrough — Microphone pane has no '+' button ([cfdaf34](https://github.com/Meridiona/meridian/commit/cfdaf3418bfaee5baca861e387f75e8b3b725e1f))
* **install:** don't abort MLX installer when MERIDIAN_OO_AUTH is unset ([3255f62](https://github.com/Meridiona/meridian/commit/3255f6224f3f109f1b9a6283b18e8a93834326fe))
* **install:** find latest OO release that actually has darwin binaries ([46ac6a7](https://github.com/Meridiona/meridian/commit/46ac6a7c5e8d7056ed03342f07dadeac5eb9a522))
* **install:** improve MLX server startup process and timeout handling ([1748b5c](https://github.com/Meridiona/meridian/commit/1748b5c20ffb71271999e56e716c9aa16036f3ff))
* **install:** make meridian stop actually stop daemons under KeepAlive ([eec56da](https://github.com/Meridiona/meridian/commit/eec56da1896e2b4353e0e46bb5e27b848efb48d5))
* **install:** pin screenpipe to 0.3.350 via npm ([62c9973](https://github.com/Meridiona/meridian/commit/62c99735e284f852f2005882ec67e28c03a76f30))
* **install:** prompt for OO email+password separately instead of raw base64 ([27ef7c4](https://github.com/Meridiona/meridian/commit/27ef7c4e375dbdfe9f00f3cf0275f854580d136a))
* **install:** prompt for OO email+password separately instead of raw base64 ([fbf08b6](https://github.com/Meridiona/meridian/commit/fbf08b66bd91befb47989e06f14272226a031696))
* **install:** reduce MLX readiness probe to 60s; don't block on first-run download ([0e2eebc](https://github.com/Meridiona/meridian/commit/0e2eebc0afe3a50526ac1720c8673a9f3763abc4))
* **install:** run pip install -e . so agents package is importable after setup ([43184b3](https://github.com/Meridiona/meridian/commit/43184b336367c3ead0ceb31a3409ce4d45aca8ce))
* **install:** seed services/.hermes/ before env collection + switch screenpipe to npm ([3d07cb6](https://github.com/Meridiona/meridian/commit/3d07cb6a4bfb456cd5ac0c7a32fadd20c025a08b))
* **install:** stream MLX log on first run; fast probe when model is cached ([32b4992](https://github.com/Meridiona/meridian/commit/32b49926ba53eaebc06dff7a515329215f2b4f79))
* **install:** sync shared env vars and pin screenpipe to ~0.3 ([06ed055](https://github.com/Meridiona/meridian/commit/06ed0558f85d642a7891134e2dd1b08c89c19cc3))
* **install:** update screenpipe install hint from brew to npm ([f3f3a80](https://github.com/Meridiona/meridian/commit/f3f3a8061027c1c8894bb20cb43ce1712816f6b9))
* **install:** update screenpipe install hint in install-screenpipe-daemon.sh ([d46a3fd](https://github.com/Meridiona/meridian/commit/d46a3fd04024631936805acc1e9243890757441c))
* **install:** wait for bootout before bootstrap in daemon/ui/screenpipe installers ([7ec084f](https://github.com/Meridiona/meridian/commit/7ec084f9a3d045441c7db508ca3057a09172c148))
* **install:** wait for launchd bootout before MLX bootstrap (avoid EIO) ([7b007bb](https://github.com/Meridiona/meridian/commit/7b007bb1f98c1975b6871e933f9154b5f197d9eb))
* **intelligence:** address code review findings across Rust backend ([8bdcf8f](https://github.com/Meridiona/meridian/commit/8bdcf8fe53455e7c9dba7a887add9f60a7bdef5d))
* **intelligence:** category settler skips historical sessions on first run ([ea03cc1](https://github.com/Meridiona/meridian/commit/ea03cc1e203f7137193b8e16c2783571ac7ec4da))
* **intelligence:** classify all new browser sessions immediately, skip backlog ([f7f251f](https://github.com/Meridiona/meridian/commit/f7f251fdf0996e62c07b2687cff9df7c2ace28da))
* **intelligence:** classify all new browser sessions immediately, skip backlog ([22999f3](https://github.com/Meridiona/meridian/commit/22999f326139d834c8ee66f8161e0571a5e7cb67))
* **intelligence:** embed skill inline and disable toolsets to fix parse failures ([47ba668](https://github.com/Meridiona/meridian/commit/47ba66894e8a45e0cdbb71a1efb912a90dac0853))
* **intelligence:** fix MLX deprecated API, fast-fail on server exit, add psutil+mlx-lm deps ([fedcf5b](https://github.com/Meridiona/meridian/commit/fedcf5bfeaf9cd77ae8917d7e2297673bb4f9c3a))
* **intelligence:** harden Chrome category settler and add tests ([4682d12](https://github.com/Meridiona/meridian/commit/4682d124475b2c86a1e62a7792f00ae79f97a4db))
* **intelligence:** harden Chrome category settler and add tests ([de23ea3](https://github.com/Meridiona/meridian/commit/de23ea3130ec53b05e2b45b08a2df12a9160eae1))
* **intelligence:** log Foundation Models skip as warn instead of silently swallowing ([633a85e](https://github.com/Meridiona/meridian/commit/633a85e407e6dc14819cc2df8b8d9a41883ae851))
* **intelligence:** log Foundation Models skip as warn instead of silently swallowing ([2e05d7b](https://github.com/Meridiona/meridian/commit/2e05d7b445ec750df0075a86d8128acad437637e))
* **intelligence:** promote run_task_linker python stderr from debug to info ([cdd858d](https://github.com/Meridiona/meridian/commit/cdd858dcbb9879407eb9dc74b1535548c2089ab0))
* **intelligence:** skip backlog, lower browser session threshold to 5s ([46a0e66](https://github.com/Meridiona/meridian/commit/46a0e66b0b2b4d45c66bebf866bf6e2027cd6bff))
* **intelligence:** skip backlog, lower browser session threshold to 5s ([a24e220](https://github.com/Meridiona/meridian/commit/a24e22056ec0cae9be808fd3fe9c1bdee33b4cd0))
* **intelligence:** strip non-Latin characters from FM prompts ([17cf377](https://github.com/Meridiona/meridian/commit/17cf377cc32531c3086b145292c2c336fa93d603))
* **intelligence:** strip non-Latin characters from FM prompts ([ff00332](https://github.com/Meridiona/meridian/commit/ff003323fb31d4fb8895acea4cd4ec1ba8d7d28f))
* **intelligence:** write sentinel on permanent FM failures to prevent endless retry ([3480092](https://github.com/Meridiona/meridian/commit/34800929cb62eeeb34ce3dd44c77466bb7faf9af))
* **intelligence:** write sentinel on permanent FM failures to prevent endless retry ([395ea8d](https://github.com/Meridiona/meridian/commit/395ea8dceab5f8e66f127f6180b163ef7b0b032f))
* **launchd:** apply hardened bootout/enable pattern to install-daemon.sh ([53aeb70](https://github.com/Meridiona/meridian/commit/53aeb7021e62db9e7dcb2fa1b94349f6c4ca5f4f))
* **launchd:** harden all daemon installers against bootstrap failures ([a0c123e](https://github.com/Meridiona/meridian/commit/a0c123ebde52d996ca0e896b45bacb1267fbce87))
* **llm-selector:** detect Ollama installed models and install mlx-lm on Apple Silicon ([745b5b9](https://github.com/Meridiona/meridian/commit/745b5b90410bf82c1e490b9cb395f778ed230ec8))
* **llm-selector:** read LLM_BUDGET_PCT from config when budget_pct not passed ([c7051f3](https://github.com/Meridiona/meridian/commit/c7051f3723542b83d12a43e4d81d9b798efc64f5))
* **llm-selector:** stabilise model selection by adjusting headroom for running managed server ([030bc7c](https://github.com/Meridiona/meridian/commit/030bc7c3f22ede3efdb0d66743d1d3afcb3927a6))
* **llm-selector:** stabilise model selection by adjusting headroom for running managed server ([e689a16](https://github.com/Meridiona/meridian/commit/e689a1672d774dec2983ee1542fb28c92e40c344))
* **llm-selector:** use LM Studio /api/v0/models to detect loaded models ([00b55ec](https://github.com/Meridiona/meridian/commit/00b55ec074806c49ca132cfa31fd383de2f18274))
* **llm-selector:** wait for managed server to die before starting replacement ([890a0f4](https://github.com/Meridiona/meridian/commit/890a0f4d722ce0952a86b1a4669fe04799e49fa2))
* **llm-selector:** wait for managed server to die before starting replacement ([da4629b](https://github.com/Meridiona/meridian/commit/da4629b04981e1f58ab6305a883545310139014f))
* **mcp:** replace better-sqlite3 with sql.js to eliminate native module version errors ([b13b907](https://github.com/Meridiona/meridian/commit/b13b9071cd25c9387a088b942a245d0c04dcb96a))
* **mcp:** use local node path instead of npx until package is published to npm ([3f99326](https://github.com/Meridiona/meridian/commit/3f993266ca1b9ce2d0bb2e0dbbc218e1bbf68ff2))
* **migrations:** add category_method column to app_sessions table ([55b063b](https://github.com/Meridiona/meridian/commit/55b063b48e055da4020f61107123c845320be0e2))
* **migrations:** add category_method column to app_sessions table ([9e06cfe](https://github.com/Meridiona/meridian/commit/9e06cfe9cbd5682fbdc358dce6cbd860e5fe3571))
* **migrations:** drop and recreate pm_task_embeddings around pm_tasks swap in 022 ([5fd5fa2](https://github.com/Meridiona/meridian/commit/5fd5fa2a44a3c3d6f9a371b1705fc1990758328c))
* **mlx-server:** remove ProcessType Background; extend readiness probe to 600s ([785ab1f](https://github.com/Meridiona/meridian/commit/785ab1fe255f7090ba9b3bcab1be7cd119d45ce8))
* **observability:** disable OTel in dev to prevent Map size exhaustion ([05c7f8a](https://github.com/Meridiona/meridian/commit/05c7f8a488d910151119c58a87b3963ae2bf3006))
* **observability:** flush OTel spans before short-lived subprocess exits ([588beef](https://github.com/Meridiona/meridian/commit/588beefe0e0a11a79c5a4c1970859a7fef9729b6))
* **observability:** log to stdout so daemon.log / 'meridian logs' captures it ([110869e](https://github.com/Meridiona/meridian/commit/110869ea3f4d1767252addb49364edb02daec05f))
* **observability:** update OpenObserve installation to use fixed version v0.11.0 ([34e43b5](https://github.com/Meridiona/meridian/commit/34e43b5d02160eff7855e5008ea2deec3be1dfca))
* **observability:** use .instrument() to parent run_task_linking under poll_tick ([47fd674](https://github.com/Meridiona/meridian/commit/47fd674621671169291fd6f472df59ba4254f0c6))
* **openobserve:** cap memory, default log level to warn, fix crash-loop ([80f4bec](https://github.com/Meridiona/meridian/commit/80f4becfa75f75e23f489b140abfb756a54c37c1))
* **pm-tasks:** exclude subtasks and normalize status categories ([5a62a0c](https://github.com/Meridiona/meridian/commit/5a62a0c2ef1225d2a590491e9239c74843325bf1))
* **release:** build on macOS 26 SDK, complete FFI stub, fetch tags ([7503155](https://github.com/Meridiona/meridian/commit/75031551c8e17e60590908fd94c00c5b26addb8f))
* **release:** publish arch package by path, not npm owner/repo shorthand ([cafa1aa](https://github.com/Meridiona/meridian/commit/cafa1aa1e9b10fb78b770e903af550127e1010c1))
* **screenpipe:** enhance get_frame_full_texts to fallback on accessibility_text when full_text is NULL ([c6d2158](https://github.com/Meridiona/meridian/commit/c6d2158ae9465caa8137daea8f37494fc2e5faac))
* **screenpipe:** wrap npm script in /bin/sh for launchd compatibility ([3ff2d1a](https://github.com/Meridiona/meridian/commit/3ff2d1a1a9c3830590ddbc559d8bd0cb45dc63f7))
* **scripts:** chain coding-agent-indexer daemon and Claude hook into install scripts ([42fbac3](https://github.com/Meridiona/meridian/commit/42fbac3167e42dd2df71eece54f39114a303bbf4))
* **services:** address Python code review findings and clean up pipeline ([09f4866](https://github.com/Meridiona/meridian/commit/09f4866392cdeea2614257f91b3c0ef3541ea2b3))
* **services:** tighten Python dependency version bounds ([3ee9971](https://github.com/Meridiona/meridian/commit/3ee997107bdcddfc36af49efc003ead6e9a14b1f))
* **settings:** number stepper allows free typing without clamping mid-edit ([d43c8b0](https://github.com/Meridiona/meridian/commit/d43c8b0c5eddf21be007416f782a708bd7ad7ee8))
* **settings:** number stepper allows free typing without clamping mid-edit ([e021d98](https://github.com/Meridiona/meridian/commit/e021d9866423322b1b18674ad562397194ff50b2))
* **tagger:** correct Ollama Cloud base URL to https://ollama.com/v1 ([ada1c06](https://github.com/Meridiona/meridian/commit/ada1c060b8cc605f046558cc6e9f2c342f93496c))
* **tagger:** replace ocr_samples with session_text throughout Python services ([135de33](https://github.com/Meridiona/meridian/commit/135de334f51015c17a3cff99ccf031b11eece4bd))
* **tagger:** use Ollama Cloud env vars for Stage 3 LLM config ([d07d3c9](https://github.com/Meridiona/meridian/commit/d07d3c944b5b6fc4705f2d0393e4a0d7bb3e5739))
* **task-classifier:** clarify session_type logic and confidence scoring ranges ([48bf82d](https://github.com/Meridiona/meridian/commit/48bf82d02f3a6e6d820333947aabe41fef2f89b2))
* **task-classifier:** correct example session_type from overhead to unknown ([ea43d88](https://github.com/Meridiona/meridian/commit/ea43d888414327081749b5f8c052d52657beb8ba))
* **task-classifier:** intermediate sessions should link to task if work-related to prior context ([cab5185](https://github.com/Meridiona/meridian/commit/cab5185fb98b6d4202529d37900fd86a235f8f03))
* **task-classifier:** raise confidence for clear work signals without task mapping (0.6-0.8) ([58cd4dd](https://github.com/Meridiona/meridian/commit/58cd4dde42c593e6d27057e2b61ccd6794de5ebb))
* **task-classifier:** require work alignment for high confidence, not just task key visibility ([6042878](https://github.com/Meridiona/meridian/commit/60428787ae34eed1636ab6e28cc32c0cf536317c))
* **task-classifier:** standardize session_type naming to 'untracked' throughout ([3494bb3](https://github.com/Meridiona/meridian/commit/3494bb33a58a00e69c1bafadc09b6984f5007a79))
* **task-linker:** add reasoning and session_type to classification debug logs ([486074b](https://github.com/Meridiona/meridian/commit/486074baee76aad9750d9c7e4ffe2eed03ccb6c7))
* **task-linker:** reduce batch limit to 1 session per daemon tick ([8b66354](https://github.com/Meridiona/meridian/commit/8b663543f78205bcd0ff14ea13279c0bc2e0463c))
* **task-linker:** remove unused fetch_sessions_by_ids ([06baaaf](https://github.com/Meridiona/meridian/commit/06baaaf652bb960b34de72ff5c4fef41430febbe))
* **task-linker:** resolve post-merge protocol mismatch and import errors ([db98586](https://github.com/Meridiona/meridian/commit/db98586bee3c28d797129fbceef9060e6df07477))
* **tests:** add accessibility_text column to screenpipe mock schema ([134e78c](https://github.com/Meridiona/meridian/commit/134e78cd005c7b2db4523e5491b006a962dc09f2))
* **tests:** add missing accessibility_text column to test frames schema ([6a34d02](https://github.com/Meridiona/meridian/commit/6a34d02a029ce7340c78597f777f1e9d9fbceda0))
* **tests:** enhance parse_category tests with alias handling and verbose prose fallback ([a1c963b](https://github.com/Meridiona/meridian/commit/a1c963ba1712176aba035a8f91647cbe2eacbae0))
* **tests:** mark LLM smoke tests as ignored to unblock pre-push ([a6f8016](https://github.com/Meridiona/meridian/commit/a6f80161c263d952b8ff4f56c8cf06244264f632))
* **tests:** remove stale stage-flag tests and fix MCP summary regex ([6b7b6fc](https://github.com/Meridiona/meridian/commit/6b7b6fc2f768bb93d2ce2f9bc172ea61519652f0))
* **tests:** serialise LLM smoke tests to prevent parallel timeout ([eb6315a](https://github.com/Meridiona/meridian/commit/eb6315a5f8df5320a8fd390067369c088782bb5c))
* **tests:** serialize config env-var tests with a mutex ([0d87772](https://github.com/Meridiona/meridian/commit/0d87772008bbd549edc140207ab98c75827970d2))
* **timeline:** guard NaN in CSS position calculations ([817da6a](https://github.com/Meridiona/meridian/commit/817da6abcfd47fefe7bbae47140771f9e5c139ed))
* **ui:** correct meridiona typo to Meridiona in app metadata ([331f456](https://github.com/Meridiona/meridian/commit/331f45646673f9abbebc5157f9c230dbf45482b1))
* **ui:** guard NaN in tooltip left from focus event clientX ([ab68ad8](https://github.com/Meridiona/meridian/commit/ab68ad869ac0513286cbf68207c130de41b15a45))
* **ui:** make All Time label black for readability ([2e20dce](https://github.com/Meridiona/meridian/commit/2e20dcec7e677cc4b2e9d1efcdd00fe9f9aacae2))
* **ui:** pin exact package versions, fix typescript@5.8.2 (5.8.0 was never published) ([4946f7b](https://github.com/Meridiona/meridian/commit/4946f7b4629f0adfb5d9b90c633d5176a6708df1))
* **ui:** resolve hydration mismatch in DayTimeline + quiet terminal logs ([eb1b8e8](https://github.com/Meridiona/meridian/commit/eb1b8e8fc171d25fb86ba2fe812d29f39902fb13))
* **ui:** restore correct page.tsx and remove stale ocr_samples refs ([5e2b2ce](https://github.com/Meridiona/meridian/commit/5e2b2ce6be868ce17a7cff93ee62a680619fc346))
* **ui:** switch Instrument Serif to Google Fonts CDN link ([05181ca](https://github.com/Meridiona/meridian/commit/05181cabf6071ce91a7df19b621db63416da1926))
* **ui:** update TaskBadge pipeline display for hermes methods ([ee705c7](https://github.com/Meridiona/meridian/commit/ee705c771dc9efc54e3edeef8ce9b59c776b7269))
* update parameters in generate_category method to avoid unused variable warnings ([aa44c5a](https://github.com/Meridiona/meridian/commit/aa44c5ae6b1b324d4beffe5824a58d0ef84fa9c0))


### Features

* **a11y:** add ax-sidecar to inject VS Code terminal content into screenpipe ([75827a3](https://github.com/Meridiona/meridian/commit/75827a3749beebddf4af03476f8c9746d09bc548))
* add daemon entrypoint, config, and lib crate split ([dca4751](https://github.com/Meridiona/meridian/commit/dca475150164262b01d7a7904d30f2e334eeb500))
* add PM update workflow and session summaries ([c53dc79](https://github.com/Meridiona/meridian/commit/c53dc793cde1d41fa0ccd6d4207738cd5b1ad77e))
* **agents:** add Jira update agent with slot-based scheduler ([26e343b](https://github.com/Meridiona/meridian/commit/26e343bd0c60a2be9d316168bb27bf730d4bac58))
* **agents:** implement dynamic LLM selection and download logic based on machine specs ([24976f8](https://github.com/Meridiona/meridian/commit/24976f828918a665c9b9920992192a03beb355af))
* **agents:** implement dynamic LLM selection for Apple Silicon and update configuration ([0a24667](https://github.com/Meridiona/meridian/commit/0a24667f7b1ebb4d5c6673dcb0f0f0c535ea4148))
* **ax-sidecar:** add Codex session support + tag assistant role per agent ([997fd84](https://github.com/Meridiona/meridian/commit/997fd84c182886ca4b1a31711b6541c5f97f7f39))
* **ax-sidecar:** capture Claude Code session transcripts into screenpipe ([cf8757e](https://github.com/Meridiona/meridian/commit/cf8757eb78e6308edf26c1ef53d78719375d6e93))
* **ax-sidecar:** distinguish Claude windows via session title + uuid in window_name ([66e0413](https://github.com/Meridiona/meridian/commit/66e0413ff7b47c151193038849bb851f3e7723ba))
* **ax-sidecar:** pin host app per session, anchored to session start ([e36fee3](https://github.com/Meridiona/meridian/commit/e36fee3b76fcfa03185ef35294d196c5c6c3551d))
* **ax-sidecar:** resolve app_name from screenpipe instead of hardcoding it ([902d9fc](https://github.com/Meridiona/meridian/commit/902d9fcb750be4e7aabb8e19ec5a97f0520c8413))
* **backfill:** add standalone backfill scripts for categories and task links ([02b823c](https://github.com/Meridiona/meridian/commit/02b823c7d684227304a4552bd0370a10402b8b3c))
* **classifier:** log run smoke_20260528T162251 + declare failure_class schema ([f655b96](https://github.com/Meridiona/meridian/commit/f655b9638e588282f5c741116621ea59d8683194))
* **classifier:** log run smoke_20260528T180202 — SESSION_TEXT_CAP=0 experiment ([13dd636](https://github.com/Meridiona/meridian/commit/13dd6368353c4e5e34b837f6c83cf165eddddc4d))
* **coding-agent:** port indexer + summariser into the Rust daemon ([f5da20f](https://github.com/Meridiona/meridian/commit/f5da20fb37a74e133755fef0decbc2fa671ab6c0))
* **config:** multi-provider PM config architecture ([db29400](https://github.com/Meridiona/meridian/commit/db29400106862ac957fae72c51a57e4280e9d3d3))
* **daemon:** restore sequential run_task_linking for non-MLX backends ([ca4bdf7](https://github.com/Meridiona/meridian/commit/ca4bdf7520d91b9b470bc6d785f78eae736b1324))
* **db:** add screenpipe read layer and meridian write schema ([73d9183](https://github.com/Meridiona/meridian/commit/73d9183c5b3aa15502a71bde434e2a49f8ec03e2))
* **dist:** npm distribution like screenpipe (@meridiona/meridian) ([5fa278f](https://github.com/Meridiona/meridian/commit/5fa278f42f2cc28b2a5060eeba4e7b11b2242d60))
* **etl:** add block context extractor and ETL runner ([75f8783](https://github.com/Meridiona/meridian/commit/75f87830cd0093add56a39ce6d15de314239f731))
* **etl:** add structured debug tracing across gap detection, block ops, and context extraction ([a93faac](https://github.com/Meridiona/meridian/commit/a93faacf64fc711827ccb59eab3d81a18f9dcaf3))
* **etl:** categorize sessions inline during ETL, store in app_sessions ([e678541](https://github.com/Meridiona/meridian/commit/e678541ac87c286105908ed48ceac7e8cb65eb80))
* **etl:** filter OCR noise from session_text at storage time ([f3e04f2](https://github.com/Meridiona/meridian/commit/f3e04f2104074308046ba4447d120dfebdefcbdb))
* **etl:** gap detection — separate gaps table, system_sleep vs user_idle ([dbf9dc9](https://github.com/Meridiona/meridian/commit/dbf9dc9934d7ed285b8b1457c0bb28ee9a2c739d))
* **etl:** infer Antigravity from Terminal OCR, fix browser domain grouping ([4ef62c0](https://github.com/Meridiona/meridian/commit/4ef62c0c13e799966eb58412bc33a1cb2d7f75a5))
* **etl:** Option C — refine session end using ui_events click/key timestamps ([693a454](https://github.com/Meridiona/meridian/commit/693a454b3f6acc2d6d6784d540d5857839690d19))
* **etl:** split browser sessions by domain, add cat_smoke diagnostic binary ([6b25314](https://github.com/Meridiona/meridian/commit/6b253146d7c85c7de26c470fafc4cf9e7c2d50c8))
* **etl:** split VS Code sessions by project and dedup terminal keystrokes ([9586209](https://github.com/Meridiona/meridian/commit/9586209ed5a76e161fda610ff35ef32251399f8c))
* **eval-feedback:** read local results JSON instead of querying OpenObserve ([712bcd2](https://github.com/Meridiona/meridian/commit/712bcd2e92d02f9e1fceb005f8ec1e1e06d708df))
* **eval:** dynamic LLM selector — /info endpoint + auto-discovery + --model flag ([0f2b87c](https://github.com/Meridiona/meridian/commit/0f2b87c5e17cb25a85cca8a7a03d3a3e9b959d36)), closes [task#1](https://github.com/task/issues/1) [task#1](https://github.com/task/issues/1)
* **evals:** --config flag for versioned experiment manifests ([ff238c8](https://github.com/Meridiona/meridian/commit/ff238c8875d9ea3031d2cfc0d3d4295f83b09381))
* **evals:** add Dev A 35-session golden seed dataset ([a2c10f5](https://github.com/Meridiona/meridian/commit/a2c10f5aba3408f35f2b2bd8c2d7659c48fa3f58))
* **evals:** add Dev A candidates with 5 real KAN tickets + 2 decoys ([63a38ca](https://github.com/Meridiona/meridian/commit/63a38caf44065cc09f34b2cd3c7f98b89e9a2e08))
* **evals:** add Dev B generic persona dataset — sessions 1-15 of 35 ([3e98004](https://github.com/Meridiona/meridian/commit/3e980040ad3bc4550bb6b65478155b63d075690b))
* **evals:** add EvalStrategy abstraction for pluggable classifier configs ([bd87c42](https://github.com/Meridiona/meridian/commit/bd87c42cdd6851baea1e19e28691ad407813c59b))
* **evals:** add MLX classifier evaluation suite with DeepEval ([eb62d49](https://github.com/Meridiona/meridian/commit/eb62d491c8efd1f1d0fd38c77f484408e0b9d497))
* **evals:** add overhead + untracked tiers to b_generic dataset ([133ccb6](https://github.com/Meridiona/meridian/commit/133ccb60a072d799507460e15607c96f6976c35f))
* **evals:** add render_seeds.py + smoke_run.py for the classifier eval pipeline ([722499a](https://github.com/Meridiona/meridian/commit/722499abb18f062138c3e223b8bc7fc05097057f))
* **evals:** Dev B sessions 16-20 — PROJ-201 completion and PROJ-225 profiling ([f50e600](https://github.com/Meridiona/meridian/commit/f50e600407c6992a7039412c387980f40aaa6210)), closes [#88](https://github.com/Meridiona/meridian/issues/88) [#94](https://github.com/Meridiona/meridian/issues/94) [#frontend](https://github.com/Meridiona/meridian/issues/frontend)
* **evals:** Dev B sessions 21-25 — hard discrimination cases and PROJ-230 ([9163e5d](https://github.com/Meridiona/meridian/commit/9163e5d59064ccd8e3974868fdd63ed4df91a001)), closes [#92](https://github.com/Meridiona/meridian/issues/92) [#93](https://github.com/Meridiona/meridian/issues/93) [#92](https://github.com/Meridiona/meridian/issues/92) [#95](https://github.com/Meridiona/meridian/issues/95)
* **evals:** Dev B sessions 26-30 — bug fix, scope control, and PROJ-242 deference ([7eee871](https://github.com/Meridiona/meridian/commit/7eee87127c1cd3190ef5349a6a2e9f801bbb1d6e)), closes [#94](https://github.com/Meridiona/meridian/issues/94) [#88](https://github.com/Meridiona/meridian/issues/88) [#96](https://github.com/Meridiona/meridian/issues/96)
* **evals:** Dev B sessions 31-35 — complete generic persona dataset (35/35) ([fe926ad](https://github.com/Meridiona/meridian/commit/fe926ad3673c691f5e956e892278f13f3dfd18f3))
* **evals:** experiment config for extract_then_classify on Dev B ([d481fbb](https://github.com/Meridiona/meridian/commit/d481fbb17385e4594bcedc0b213d91b5ce0dc462))
* **evals:** ExtractThenClassifyStrategy — two-stage classification (Task [#13](https://github.com/Meridiona/meridian/issues/13)) ([54a469f](https://github.com/Meridiona/meridian/commit/54a469f96f865ae3c3cc6164ecd11e1e6204e729))
* **evals:** log eval run qwen35-9b-optiq-4bit-b_generic-baseline-20260529 ([9cf31ff](https://github.com/Meridiona/meridian/commit/9cf31ff487567eb623d701c72d006796e4477e54))
* **evals:** log run b_generic_direct_http_20260530T110547 to FEEDBACK ([b02ea61](https://github.com/Meridiona/meridian/commit/b02ea6195b6b3911c988497144a2976ae543d7d0))
* **evals:** log run b_generic_extract_then_classify_20260530T190121 to FEEDBACK ([0cfa02e](https://github.com/Meridiona/meridian/commit/0cfa02e82c52598fd87778d227fdf6b84e302d89))
* **evals:** meaningful run_id + richer OO trace coverage ([89043da](https://github.com/Meridiona/meridian/commit/89043da0e603b456e8b5bbc8d59866ab3fe63ed3))
* **evals:** nested OO traces for ExtractThenClassifyStrategy stages ([2abb880](https://github.com/Meridiona/meridian/commit/2abb880c7135ef4b32a0a6eb3cf216f336751302))
* **evals:** write canonical results JSON per eval run ([2d40c5e](https://github.com/Meridiona/meridian/commit/2d40c5e726e7ff0ec1d4248be8f351dbe87e4041))
* **install:** add --mlx flag to install.sh for persistent MLX server ([899cc5a](https://github.com/Meridiona/meridian/commit/899cc5aaf007aea3c9f5ecf8ebc33adfb23b9a7b))
* **install:** add OpenObserve as an optional prereq ([2f62758](https://github.com/Meridiona/meridian/commit/2f627585e3c1c7921de00f05efdb26245f523a4b))
* **install:** auto-start OpenObserve as a launchd LaunchAgent ([7e0fde0](https://github.com/Meridiona/meridian/commit/7e0fde0c729c10496135e3e9c5a1930ec1e091e6))
* **install:** interactive credential collection + install-package test suite ([e04ba3e](https://github.com/Meridiona/meridian/commit/e04ba3ee3d5d189cd936ceed6abc01b7c00ba977))
* **install:** manage the Next.js dashboard as a launchd LaunchAgent ([dd32acc](https://github.com/Meridiona/meridian/commit/dd32acccbd7e65892d43a4c5d9390c143179d826))
* **install:** MLX default, single repo .env, retire jira-updater ([1fc93c7](https://github.com/Meridiona/meridian/commit/1fc93c78414f50c9dab8ca3d3c0ffbd6d483c689))
* **install:** one-command install with launchd-managed screenpipe, meridian daemon, and jira-updater ([2342b46](https://github.com/Meridiona/meridian/commit/2342b4662d26e64b1e18313f6d9290aadbf260e8))
* **install:** one-command prebuilt installer (no clone, no build) ([7837381](https://github.com/Meridiona/meridian/commit/78373813614d8da68fc69ce4253e09946410a505))
* **intelligence:** add backfill binaries for session categories and task classification ([69606d9](https://github.com/Meridiona/meridian/commit/69606d9586011ce6aeb5f69aeece38743bc05758))
* **intelligence:** add development category and include session_text in FM category prompt ([f27ebac](https://github.com/Meridiona/meridian/commit/f27ebaceb8c8a9f96c7b1755a4793e5f84119087))
* **intelligence:** add dynamic local LLM selection for task classifier ([ad3fbc2](https://github.com/Meridiona/meridian/commit/ad3fbc24bbb244704d131bc94bedf97d08bd7120))
* **intelligence:** add LLM classifier pipeline with Foundation Models support ([9c53223](https://github.com/Meridiona/meridian/commit/9c53223061157835ca76b6ac13ab2acf794e25c1))
* **intelligence:** add LLM classifier pipeline with Foundation Models support ([c088e00](https://github.com/Meridiona/meridian/commit/c088e00d6a8bdd2e99d748bd192376ad7fb65971))
* **intelligence:** add pm_tasks and ticket_links migrations ([2d0b363](https://github.com/Meridiona/meridian/commit/2d0b363b69d67d35653b0462d328a99566c12c56))
* **intelligence:** add Swift Foundation Models FFI bridge ([368dbe8](https://github.com/Meridiona/meridian/commit/368dbe804a42b10cb1ac7dcbc0596000b8717351))
* **intelligence:** add Swift Foundation Models FFI bridge ([2017ec2](https://github.com/Meridiona/meridian/commit/2017ec2020714bd0ae7b6ce787ebd162e5b1ee28))
* **intelligence:** add task-key debug logging to PM provider sync ([9c421c2](https://github.com/Meridiona/meridian/commit/9c421c2f1916d25053130f7e9067ad44a75fbb05))
* **intelligence:** add tracing instrumentation to jira provider ([313a414](https://github.com/Meridiona/meridian/commit/313a41408d19503c573760b77aa2a7d7e2629151))
* **intelligence:** distinguish category confirmed vs updated in settler logs ([9e63381](https://github.com/Meridiona/meridian/commit/9e6338127b63d559cd77f1038c7c73a195abab6f))
* **intelligence:** enforce FM structured output via @Generable for category classification ([1bdf7f0](https://github.com/Meridiona/meridian/commit/1bdf7f0ab7e54a4407fcef686d2e113d35d2422d))
* **intelligence:** expand FM category classification to all apps ([acf2425](https://github.com/Meridiona/meridian/commit/acf2425cf2ad5ff9227845843df5eee6a5d05884))
* **intelligence:** Jira REST connector and provider stubs ([2d2cc94](https://github.com/Meridiona/meridian/commit/2d2cc94584cea4ca02631081fa80c99e1ebdf077))
* **intelligence:** propagate run_fm_categorization rename and add tracing spans ([ce75135](https://github.com/Meridiona/meridian/commit/ce7513585f5a884cf6bdba2bccd862d27539053f))
* **intelligence:** session activity categorizer with 86 unit tests ([c3f4d1c](https://github.com/Meridiona/meridian/commit/c3f4d1cc6a3d35fe950383afe3997040ba32e505))
* **intelligence:** wire dynamic LLM endpoint into hermes AIAgent ([7169143](https://github.com/Meridiona/meridian/commit/7169143d11cdabfd61b0325b868dcd8b25309d46))
* **intelligence:** wire into daemon, add reqwest and dotenvy ([c75a3f4](https://github.com/Meridiona/meridian/commit/c75a3f4d5c16be5d78528ff3d897f5693cd0c786))
* **launchd:** add MLX server daemon scripts ([82e9d95](https://github.com/Meridiona/meridian/commit/82e9d957a3f87e927a54102ad09116a0c9186a07))
* **llm-gate:** add process-global single-permit LLM gate ([025ee53](https://github.com/Meridiona/meridian/commit/025ee5368c2adca413f836f78ab91dd1bb447dc2))
* **llm-gate:** serialise classify + summarise MLX calls through the gate ([d4d0fc1](https://github.com/Meridiona/meridian/commit/d4d0fc126eec40da80bc5db00f700cac1f96a80d))
* **llm-selector:** add comprehensive logging and tracing to model selection ([be4a1dd](https://github.com/Meridiona/meridian/commit/be4a1ddaed28b3acab5f89d1c875197ed34a9608))
* **llm-selector:** add comprehensive logging and tracing to model selection ([4c08b70](https://github.com/Meridiona/meridian/commit/4c08b709606f45bde775162c3874c07a50d2ee53))
* **llm-selector:** auto-unload managed mlx server when external server detected ([1b47393](https://github.com/Meridiona/meridian/commit/1b47393a86cb4447c1c74a908a2779f26f4626bd))
* **logs:** split errors into a separate stream/command (daemon + MLX) ([f2b6104](https://github.com/Meridiona/meridian/commit/f2b6104fa5412463c8279fd28bd845d170e6fbf9))
* **main:** enable browser category settler after each ETL pass ([6722bda](https://github.com/Meridiona/meridian/commit/6722bda7cb97801bcba2ccae139eb21ae922fd19))
* **main:** enable browser category settler after each ETL pass ([5591021](https://github.com/Meridiona/meridian/commit/55910210be55f947f112c8193af6004a5bb096fe))
* **mcp:** add npx installer that auto-configures Claude Desktop and Cursor ([ff29db9](https://github.com/Meridiona/meridian/commit/ff29db986e75cc1148a0112644354c9a4f66a7b0))
* **mcp:** add production-ready Meridian MCP server ([cd341bd](https://github.com/Meridiona/meridian/commit/cd341bd945b9f4467c3679db7ee8cbab6804effc))
* **mcp:** install into Claude Code CLI — user and project level ([594e6c6](https://github.com/Meridiona/meridian/commit/594e6c6ea2b12fb7154dcb337797fb37956593d1))
* **mcp:** remove audio_snippets from LLM-facing tool responses ([e186aa9](https://github.com/Meridiona/meridian/commit/e186aa972b284847b703d2007423677497304dff))
* **mcp:** remove ocr/elements from queries, expose session_text ([0846e9d](https://github.com/Meridiona/meridian/commit/0846e9dbd9ab34bb80c434d1897d226dc983c159))
* **meridian-agents:** adapt db.py to 005 schema with new helpers ([4d6c080](https://github.com/Meridiona/meridian/commit/4d6c080a6abf2ce05c26537f123c6a59bad39c2d))
* **meridian-agents:** adapt db.py to 005 schema with new helpers ([40348bb](https://github.com/Meridiona/meridian/commit/40348bb0a82ddcb36a5e2b140606e88e36aa0676))
* **meridian-agents:** add llm.py — async wrapper around hermes AIAgent ([6a1f30e](https://github.com/Meridiona/meridian/commit/6a1f30e59da9597629e1d9e2fa99b58f3d250aa1))
* **meridian-agents:** add llm.py — async wrapper around hermes AIAgent ([de70eda](https://github.com/Meridiona/meridian/commit/de70edab30246257038b8bf919f80550233ae5a9))
* **meridian-agents:** config and db layers with full test suite ([13eb999](https://github.com/Meridiona/meridian/commit/13eb999ab9d9429e289f77d6aba1e120c4cb5dd8))
* **meridian-agents:** config and db layers with full test suite ([3102720](https://github.com/Meridiona/meridian/commit/3102720bfe9813944eb82aafbbca6288589386fd))
* **meridian-agents:** event-driven tagger via long-running daemon ([c8a8f68](https://github.com/Meridiona/meridian/commit/c8a8f68197b81abf822b045e58e03b0a2e897e90)), closes [hi#water-mark](https://github.com/hi/issues/water-mark)
* **meridian-agents:** event-driven tagger via long-running daemon ([210201b](https://github.com/Meridiona/meridian/commit/210201b0f5e2bea98dd1ff722cc7ab2d1eb0006e)), closes [hi#water-mark](https://github.com/hi/issues/water-mark)
* **meridian-agents:** hot-toggle stages without restarting the daemon ([07b874f](https://github.com/Meridiona/meridian/commit/07b874faea1bea01617ed263556c9f101d1c2326))
* **meridian-agents:** hot-toggle stages without restarting the daemon ([0da3d13](https://github.com/Meridiona/meridian/commit/0da3d13fac1b0597d33ac8e137459eb56b9c0131))
* **meridian-agents:** integrate synthesizer with meridian.db ([177515b](https://github.com/Meridiona/meridian/commit/177515b0888a77da14eae08348309269de40fa4e))
* **meridian-agents:** integrate synthesizer with meridian.db ([29dc01e](https://github.com/Meridiona/meridian/commit/29dc01e05002464ff0813a5306c75070ebcbf1f0))
* **meridian-agents:** parallelize synthesizer tag phase ([37d0907](https://github.com/Meridiona/meridian/commit/37d0907ca9bb46b4ecf74b779d894bfe8c0d87df))
* **meridian-agents:** parallelize synthesizer tag phase ([c34131c](https://github.com/Meridiona/meridian/commit/c34131c0af5c0f98f618146a9c5710aa72286c0d))
* **meridian-agents:** per-stage enable/disable flags ([3ffa928](https://github.com/Meridiona/meridian/commit/3ffa928475bed256fcc6e18ca73d34636b3ba840))
* **meridian-agents:** per-stage enable/disable flags ([79dc1bc](https://github.com/Meridiona/meridian/commit/79dc1bc93e5a4b3d052e31de354e182c04a91690))
* **meridian-agents:** tagger single-session inspection mode ([dfbe13d](https://github.com/Meridiona/meridian/commit/dfbe13d7559905d4acb9f2d4e1b63b78473d7dfb))
* **meridian-agents:** tagger single-session inspection mode ([cae1d9c](https://github.com/Meridiona/meridian/commit/cae1d9c011ee3134203a368724137616696cc97e))
* **meridian-agents:** tagger Stage-1 — multi-dimensional rules ([0d32ea5](https://github.com/Meridiona/meridian/commit/0d32ea5eb89f88024f916f3476459e47fa7584a5))
* **meridian-agents:** tagger Stage-1 — multi-dimensional rules ([5ddfefe](https://github.com/Meridiona/meridian/commit/5ddfefe4b82c4b63d13cc626d51745001a62721b))
* **meridian-agents:** tagger Stage-2 — embedding-based ticket matcher ([dd09775](https://github.com/Meridiona/meridian/commit/dd097756b5cf86b81b8c09b1659d4546745f625b))
* **meridian-agents:** tagger Stage-2 — embedding-based ticket matcher ([f93ad18](https://github.com/Meridiona/meridian/commit/f93ad18edccd5f1b83c6c903fd8914783b724bd0))
* **meridian-agents:** tagger Stage-2 multi-sample max-pooling ([4ac67da](https://github.com/Meridiona/meridian/commit/4ac67da821e75ab5c3e2f37fb3a2c12d09af3eb6)), closes [#10](https://github.com/Meridiona/meridian/issues/10) [#1](https://github.com/Meridiona/meridian/issues/1)
* **meridian-agents:** tagger Stage-2 multi-sample max-pooling ([e11cb63](https://github.com/Meridiona/meridian/commit/e11cb63599e0a6a88cca858086739e3a35ef67b9)), closes [#10](https://github.com/Meridiona/meridian/issues/10) [#1](https://github.com/Meridiona/meridian/issues/1)
* **meridian-agents:** tagger Stage-3 — LLM tiebreaker ([da86bf4](https://github.com/Meridiona/meridian/commit/da86bf460951c2a9cbb37af6f0a6353db61977fe))
* **meridian-agents:** tagger Stage-3 — LLM tiebreaker ([be7801a](https://github.com/Meridiona/meridian/commit/be7801a89d6d1fbc5ccc70aa5867ba1e44708f0c))
* **meridian-agents:** vendor hermes runtime so AIAgent imports clean ([211a6fe](https://github.com/Meridiona/meridian/commit/211a6fe66eb1cffe14e9bdbd8ea01451df16c603))
* **meridian-agents:** vendor hermes runtime so AIAgent imports clean ([9ae1935](https://github.com/Meridiona/meridian/commit/9ae1935fa2cd6214efdbaa5ac67bdb3c7bf038c6))
* **meridian-agents:** vendor hermes-agent v2026.5.7 in-repo ([373f7b6](https://github.com/Meridiona/meridian/commit/373f7b6c9d445f7b16a6194a8c8267ff1af28198))
* **meridian-agents:** vendor hermes-agent v2026.5.7 in-repo ([ef4816c](https://github.com/Meridiona/meridian/commit/ef4816c2e2905ea309c3065dd70eec6a9e157f87))
* **migrations:** add 004_agents.sql for meridian-agents service ([bb9f81d](https://github.com/Meridiona/meridian/commit/bb9f81d062e491b6cd28746224c057e492d578ff))
* **migrations:** add 004_agents.sql for meridian-agents service ([445756c](https://github.com/Meridiona/meridian/commit/445756c6b5595fae5b7ed726bef69dbbfbdff655))
* **migrations:** remove ocr_samples/elements_samples, add session_text ([ebdbfd7](https://github.com/Meridiona/meridian/commit/ebdbfd7bdde2b9bfa78b9c647c75cb4223ec05d4))
* **migrations:** restore 014 drop of ocr_samples/elements_samples ([a151639](https://github.com/Meridiona/meridian/commit/a151639a4b13265a462e9be61bd098c573d6464f))
* **mlx-server:** add /synthesise_worklog endpoint ([8aad1e0](https://github.com/Meridiona/meridian/commit/8aad1e0d530f5c294513c745b48ae24469cef114))
* **mlx:** add run_task_linker_mlx module for in-process inference ([e7d3152](https://github.com/Meridiona/meridian/commit/e7d315279329354a15fb387a349695995b90491d))
* **observability:** add full prompt/response text to span events for LLM debugging ([e5c6045](https://github.com/Meridiona/meridian/commit/e5c604573d31085cbb85e8c31e766f905a9be91a))
* **observability:** add llm.model/runtime/is_local to spans and log records ([50a8ba3](https://github.com/Meridiona/meridian/commit/50a8ba3b600661bc23c655830ff3d5deefa9b1f3))
* **observability:** add MERIDIAN_TRACING_DISABLED to skip OTLP exporter ([abacb4c](https://github.com/Meridiona/meridian/commit/abacb4c6083cb4ac96f599851b5546c2e55a6c8f))
* **observability:** add OTel instrumentation to run_task_linker_mlx ([844c8e6](https://github.com/Meridiona/meridian/commit/844c8e6adb049d2e23aafdd0a979672107db30b5))
* **observability:** emit info event for each trivial session in run_task_linking ([b277578](https://github.com/Meridiona/meridian/commit/b27757839ee0ffaf89ef9ae68d36d17be8f0d5a1))
* **observability:** end-to-end distributed tracing into OpenObserve ([4b4afc7](https://github.com/Meridiona/meridian/commit/4b4afc7bf99a2c9a67c3664547a6e90b885f2658))
* **observability:** end-to-end OTLP tracing via OpenObserve ([30019e8](https://github.com/Meridiona/meridian/commit/30019e8308627c39a26aa44dcc4f07b20201b683))
* **observability:** end-to-end OTLP tracing via OpenObserve ([ce783ca](https://github.com/Meridiona/meridian/commit/ce783ca6807bac9b33047c405563a74e1a7072d7))
* **observability:** enhance LLM detection and add OpenTelemetry dependencies ([3387006](https://github.com/Meridiona/meridian/commit/33870066b18b2b2f99e2688725913dfd0b382534))
* **observability:** enhance tracing and observability for task linking and ETL processes ([2f4674a](https://github.com/Meridiona/meridian/commit/2f4674ac75fcab7f825f95a41ab7d880dc858284))
* **observability:** enhance tracing configuration and context management ([c1ed6ef](https://github.com/Meridiona/meridian/commit/c1ed6ef3648f9e7b016686c8a767a13420def9e5))
* **observability:** integrate opentelemetry-appender-tracing for enhanced logging ([b01bae9](https://github.com/Meridiona/meridian/commit/b01bae92a4d63b04c1c34aa3d2d8937144c3c9bc))
* **observability:** parent run_task_linking spans under poll_tick/startup_tick ([91ae1b2](https://github.com/Meridiona/meridian/commit/91ae1b2c46b794e7488e842fa33c2fe63dcb2f73))
* **observability:** propagate Rust traceparent into MLX server classify_sessions span ([745c193](https://github.com/Meridiona/meridian/commit/745c1932562186b686e8c27f7dcefae97ac56aa4))
* **observability:** wire W3C traceparent across Rust→Python boundary and add span events ([02fe0ce](https://github.com/Meridiona/meridian/commit/02fe0ce1e3659cfcf7222723395ae1fb36fc681e))
* **pm-worklog:** add 'meridian worklog-status' command ([859c991](https://github.com/Meridiona/meridian/commit/859c991841175ca239c1dba330db2c6f5dd6db1d))
* **pm-worklog:** in-daemon Stage 4 worklog pipeline ([59e554c](https://github.com/Meridiona/meridian/commit/59e554ccfcf67d0c186d34f955250eede5b394cc))
* **pm-worklog:** wire hourly driver + CLI into the daemon ([9454f43](https://github.com/Meridiona/meridian/commit/9454f438e7c2821078ecb38db8afbe080c69902a))
* **prompts:** update SESSION_TEXT_CAP to read from environment variable for eval experiments ([95a1457](https://github.com/Meridiona/meridian/commit/95a1457a03e34d420bf3e709f5c652cb23cb86a1))
* **release:** prebuilt macOS arm64 release pipeline ([95f363a](https://github.com/Meridiona/meridian/commit/95f363a1a56a50fd4218a8fab85e806b8d06df72))
* replace ax sidecar with coding agent indexer ([f59bfbf](https://github.com/Meridiona/meridian/commit/f59bfbfaae615b1900004fc3881d52b3daacdacc))
* **rust:** replace subprocess with HTTP call to persistent MLX server ([7ac4c03](https://github.com/Meridiona/meridian/commit/7ac4c031ca3dc67457700ccf282cc6e0b5575202))
* **scripts:** add setup-services.sh for co-dev onboarding ([e316c83](https://github.com/Meridiona/meridian/commit/e316c83552e1179400e7b34302c15639cdd926f3))
* **server:** add POST /classify_sessions endpoint to MLX server ([0d13857](https://github.com/Meridiona/meridian/commit/0d13857bd195c9765ddf722fd2952ab309f8e46b))
* **session_text:** replace ocr/elements samples with full_text union ([7ffa003](https://github.com/Meridiona/meridian/commit/7ffa0036da17ceeb2761501fb24fa9606996da1b))
* **session-categorizer:** add comprehensive tracing events similar to extractor ([01d4d7f](https://github.com/Meridiona/meridian/commit/01d4d7f2bb1842675c7eff53f8a8d7dd2505fe63))
* **session-categorizer:** add detailed reasoning breakdown to logs ([9eab3a2](https://github.com/Meridiona/meridian/commit/9eab3a2f4208413c196fa5a98a5330806b46dcb8))
* **session-categorizer:** add reasoning explanation to categorization logs ([f595fbc](https://github.com/Meridiona/meridian/commit/f595fbcef30171e95740fbf30fd6fd867eceb791))
* **session-categorizer:** add score breakdown for debugging reasoning ([144db85](https://github.com/Meridiona/meridian/commit/144db8572f5fbf65bff2bbaa53f92f970309a21f))
* **settings:** Apple-style feel on Radix UI controls ([706364e](https://github.com/Meridiona/meridian/commit/706364e9294bd851729c68334a1c3e0bbdef05bc))
* **settings:** Apple-style feel on Radix UI controls ([9ff3e33](https://github.com/Meridiona/meridian/commit/9ff3e33978bc6d3bce2cdba69439eb10591c50e1))
* **settings:** Apple-style select and stepper controls ([e2bcce1](https://github.com/Meridiona/meridian/commit/e2bcce13ed9040b383c6cb97c5dbf3c521411b69))
* **settings:** embed settings as dashboard SPA view with correct theme ([f88524e](https://github.com/Meridiona/meridian/commit/f88524ebb9024ec129aee85d8e76812aeca5c0ef))
* **settings:** replace custom controls with Radix UI primitives ([70026bb](https://github.com/Meridiona/meridian/commit/70026bb20ef0e0950c4f96835f8ae8dc74de4c50))
* **settings:** replace custom controls with Radix UI primitives ([b4b0c5b](https://github.com/Meridiona/meridian/commit/b4b0c5b4adc7b8d2f006101c7bdcf259ffbc1892))
* **settings:** runtime config UI backed by ~/.meridian/settings.json ([483c98f](https://github.com/Meridiona/meridian/commit/483c98f4e68157c4f26f009f84097e3492fa620f))
* **settler:** add why field to FM category response ([f56b3ae](https://github.com/Meridiona/meridian/commit/f56b3ae39cae56b8d9a9e3ef00389639a1b37332))
* **settler:** disable OCR in category prompt, add category_smoke binary ([c6d1d63](https://github.com/Meridiona/meridian/commit/c6d1d63841ea7aa822e39d37559ef59bda2e42bf))
* **settler:** disable OCR in category prompt, add category_smoke binary ([bf225b4](https://github.com/Meridiona/meridian/commit/bf225b4c9c89e4fd327a44577923851f2b2b97d5))
* **settler:** store FM category explanation in DB and retry on unsupported language error ([a52f57a](https://github.com/Meridiona/meridian/commit/a52f57ad285b0f2484ca042fcf4b321ec189c268))
* **skills:** add eval-feedback Claude Code skill to maintain FEEDBACK.json ([7a692f0](https://github.com/Meridiona/meridian/commit/7a692f0025e725cff5fc7ab4ba00ceb508ed30f6))
* **task-linker:** add startup preflight check for classification stack ([b95d324](https://github.com/Meridiona/meridian/commit/b95d3242fe6dc9f036908d1cd2ec9d82c25e706c))
* **task-linker:** store hermes reasoning in ticket_links ([11afaf1](https://github.com/Meridiona/meridian/commit/11afaf1faee75f946490f9cee85dd3a2b181e0b6))
* **ui/api:** add today, tasks, queue-review, and week route handlers ([2d48a15](https://github.com/Meridiona/meridian/commit/2d48a150792c2f561f204717a50354c91eb53f1e))
* **ui:** add design tokens, dark mode, and theme context ([3cdb4ca](https://github.com/Meridiona/meridian/commit/3cdb4caa7f24d86250f288fbcf3268a5a74483a6))
* **ui:** add navigation shell and dashboard page ([e0c90d4](https://github.com/Meridiona/meridian/commit/e0c90d4fae0feb66bb2bf3e473a8b9932c66e6fe))
* **ui:** add Next.js 15 activity dashboard ([6a5ecf7](https://github.com/Meridiona/meridian/commit/6a5ecf744c7295a5d9096548f3bdf838f630a035))
* **ui:** add shared atom components ([1338be8](https://github.com/Meridiona/meridian/commit/1338be86f71c772f939efe3ebc5ecab194d35e6b))
* **ui:** add task classification badges with pipeline tooltip to all views ([d568bd3](https://github.com/Meridiona/meridian/commit/d568bd3655ed612ba376ec3913c3bb121832d23f))
* **ui:** add Today, Tasks, Queue, Sessions, and Week views ([f3e9faf](https://github.com/Meridiona/meridian/commit/f3e9faf8dcbdd4c12e38735908fb54a938070a4e))
* **ui:** category-aware dashboard — badges, timeline colors, breakdown chart ([f91fc6f](https://github.com/Meridiona/meridian/commit/f91fc6f9e43c343143e8127bc7897f6e0c042359))
* **ui:** load more pagination on sessions page ([15d0fd7](https://github.com/Meridiona/meridian/commit/15d0fd7f384537c5d52f1d591013f12a6ddf6f47))
* **ui:** redesign dashboard with new design system and enhanced views ([7b28633](https://github.com/Meridiona/meridian/commit/7b2863373d553ef785a62fcc505f154dec69c8f6))
* **ui:** remove ocr/elements types and components, clean up session card ([f776eca](https://github.com/Meridiona/meridian/commit/f776ecaa8d7459e7b4ce6a9392f9fe89a9102c39))
* **ui:** render gaps table — idle/sleep blocks on timeline + away stats ([f2f39cb](https://github.com/Meridiona/meridian/commit/f2f39cb3b1e17efdb59b4f0b4680066bec057cb6)), closes [#D4D1CB](https://github.com/Meridiona/meridian/issues/D4D1CB) [#C8C6C1](https://github.com/Meridiona/meridian/issues/C8C6C1)
* **ui:** replace broken Recharts tooltip with inline hover highlight on donut ([19b0322](https://github.com/Meridiona/meridian/commit/19b032267cb21d471d832cf0cb8b27761e0e2a5f))
* **ui:** session detail page at /sessions/[id] ([441d801](https://github.com/Meridiona/meridian/commit/441d801522b2b4b8b3c17a2d2a3eb877660f59b4))
* **ui:** session detail page at /sessions/[id] ([b0ac6c0](https://github.com/Meridiona/meridian/commit/b0ac6c0571d66bb96bdd198d2a148b087cd96c89))
* **ui:** show all session data — OCR, accessibility elements, audio, signals ([67c7464](https://github.com/Meridiona/meridian/commit/67c74646b3a5a7b67e9bd73167ff45d39672dc26))
* **ui:** show coding-agent time in the totals strip ([6db8557](https://github.com/Meridiona/meridian/commit/6db8557120b7bd2062dd3a2a2dfac34c73ce5ac0))
* **ui:** show Jira task on every SessionCard ([86d5a8c](https://github.com/Meridiona/meridian/commit/86d5a8ce8be5819befd166779e1cc1593a1b9803))
* **ui:** Today's Tickets tile on the dashboard ([2c8afa4](https://github.com/Meridiona/meridian/commit/2c8afa41fc85e2e0d1c4c057f8ab84f58e80de65)), closes [#E8E6E1](https://github.com/Meridiona/meridian/issues/E8E6E1)
* **ui:** Today's Tickets tile on the dashboard ([e1926fb](https://github.com/Meridiona/meridian/commit/e1926fb3f4f82c3cda59058f4b2676c70e9db7c8)), closes [#E8E6E1](https://github.com/Meridiona/meridian/issues/E8E6E1)


### Performance Improvements

* **etl:** deduplicate session data at SQL level to reduce LLM noise ([b7d5039](https://github.com/Meridiona/meridian/commit/b7d5039ad5b60bb50321f3bc0723da781bdb275e)), closes [hi#frequency](https://github.com/hi/issues/frequency)
* **etl:** eliminate redundant SELECTs and cap audio snippets ([9b6f1b4](https://github.com/Meridiona/meridian/commit/9b6f1b434904efd78eaaf615bb29900ad6fa106c))
* **etl:** increase BATCH_SIZE from 500 to 2000 for faster initial migration ([81df44d](https://github.com/Meridiona/meridian/commit/81df44d42edbba299e65e8892aab5d9a92d9f1f6))
* **skill:** teach eval-feedback to read FEEDBACK.json lean via jq ([fdec347](https://github.com/Meridiona/meridian/commit/fdec34747bca3d4f23d114f067e9645f29ab3382))


### BREAKING CHANGES

* services/meridian-agents/src/meridian_agents/db.py and
its tests still reference summary_json/activity_kind on app_sessions —
they will fail until updated in a follow-up commit. Intentional: this
migration step lands in isolation per the agreed sequencing.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
* services/meridian-agents/src/meridian_agents/db.py and
its tests still reference summary_json/activity_kind on app_sessions —
they will fail until updated in a follow-up commit. Intentional: this
migration step lands in isolation per the agreed sequencing.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>

# 1.0.0 (2026-06-01)


* feat(migrations)!: rename 004→005 and rework agent schema ([c390a91](https://github.com/Meridiona/meridian/commit/c390a91cc5fe01d06847e81e521d493c9bc70380))
* feat(migrations)!: rename 004→005 and rework agent schema ([a405be6](https://github.com/Meridiona/meridian/commit/a405be67c99b985c4a836a2ace3857b1b0c0dff9))


### Bug Fixes

* **agents:** address all PR review changes for jira-updater and meridian-mcp ([70dfb39](https://github.com/Meridiona/meridian/commit/70dfb3959da87270dbe34fec46eb43c2b7a309f1))
* **agents:** address PR [#3](https://github.com/Meridiona/meridian/issues/3) review — defer hermes setup, fix header + docstring + cursor warning ([70e6d07](https://github.com/Meridiona/meridian/commit/70e6d070732c85de3c95fdaeef9fe1d748f08083))
* **agents:** resolve critical issues in jira-updater found in code review ([2613ca6](https://github.com/Meridiona/meridian/commit/2613ca6e2f6bc48c7e00d1429479ba5f7284f4ce))
* **agents:** validate MERIDIAN_DB env var at config load to prevent prompt injection, shell injection, and path traversal ([40a7147](https://github.com/Meridiona/meridian/commit/40a7147b5cb4b4ea2106b88620940ba037723844))
* **ax-sidecar:** harden against missing deps, transient failures, and idle states ([b1e3867](https://github.com/Meridiona/meridian/commit/b1e3867017f7674adad29cebf4715758938e3407))
* **ax-sidecar:** use OS process tree for host app detection + scan subagents/ ([1b25123](https://github.com/Meridiona/meridian/commit/1b251231299273e0456bb7c5d04cab89d93c5f92))
* **bootstrap:** update synthesizer references to tagger, fix pm_tasks schema ([578fc8d](https://github.com/Meridiona/meridian/commit/578fc8de42a1510381f64694f1dc144a356a15d2))
* **categorizer:** correct misclassifications found in real session smoke test ([c1bd8f4](https://github.com/Meridiona/meridian/commit/c1bd8f4b0e716d056414d1baaaf13f5d93258fdf))
* **categorizer:** fix false-positive DevOps from YouTube/README a11y elements ([c134d70](https://github.com/Meridiona/meridian/commit/c134d7048fb13765d89248b770999322c55d1525))
* **classifier:** drop the 300-char cap on the reasoning field ([7823758](https://github.com/Meridiona/meridian/commit/782375854258e0b7087cf864257ae98152c3c392))
* **classifier:** enforce JSON output and disable Qwen3 thinking mode ([01b564b](https://github.com/Meridiona/meridian/commit/01b564b5c2e1760e728855ca3883d590a219408d))
* **classifier:** robust response extraction and JSON sanitization ([4fefe19](https://github.com/Meridiona/meridian/commit/4fefe190823d2e581021b44fc2489784328f9f27))
* **classifier:** stop outlines truncating task reasoning at 300 chars ([8dbaff7](https://github.com/Meridiona/meridian/commit/8dbaff78edeb493479eae0369456420511ae6edf))
* **cli:** add mlx-server to managed labels; kill orphaned mlx_lm.server on stop ([7139796](https://github.com/Meridiona/meridian/commit/71397967f93cfb2ad739035b9d97bb632d902693))
* **config:** disable jira updater by default, skip gracefully if module missing ([7a94a4a](https://github.com/Meridiona/meridian/commit/7a94a4a32350f1f29dcf3cc78a3e342206cf78d8))
* **css:** use direct path for tailwindcss import ([e81c7cc](https://github.com/Meridiona/meridian/commit/e81c7cc8ef58fa81a0e2eda5731f8b79a5649971))
* **db:** improve window title normalization for non-browser apps ([aa7f02a](https://github.com/Meridiona/meridian/commit/aa7f02a879a84ff50a63835808af1279a65fdfba))
* **db:** remove LIMIT 10 from get_element_samples and add TRIM dedup ([36ca205](https://github.com/Meridiona/meridian/commit/36ca205c6042bb2d530a6517a31e046b39514ef6))
* **donut:** eliminate Recharts SSR hydration mismatch ([1e269f2](https://github.com/Meridiona/meridian/commit/1e269f241b7c71c343cf32f71ce98eee320d3474))
* **etl:** close block on gap so session duration_s excludes gap time ([66e81ce](https://github.com/Meridiona/meridian/commit/66e81ce722e57dc1c1efdb5fb128b1522ee501da))
* **etl:** deduplicate AXTextArea — keep only last snapshot per session ([d653c6c](https://github.com/Meridiona/meridian/commit/d653c6c6978714b209de938af1d2a0bea6d849c3))
* **etl:** detect sleep gaps that span ETL run boundaries ([a8f2280](https://github.com/Meridiona/meridian/commit/a8f22805abe9fdc059c55dab91a6598ab67f66e9))
* **etl:** fix three gap detection bugs ([a63d0e6](https://github.com/Meridiona/meridian/commit/a63d0e62ca286ed365c675d7af6f3345f89a40f7))
* **etl:** merge reclassified Terminal sessions correctly ([7af11f8](https://github.com/Meridiona/meridian/commit/7af11f84bdfe2a2b44420f5b5ad53a127d3bdca2))
* **etl:** merge reclassified Terminal sessions correctly ([f03b61b](https://github.com/Meridiona/meridian/commit/f03b61ba2e583112f5ab8ce66d96d2e782412fac))
* **etl:** recover inter-frame gap in session duration (extend Option D) ([06bc9ab](https://github.com/Meridiona/meridian/commit/06bc9ab50f26af0db18d56027965c9c94c1a1ab0))
* **etl:** refactor block args into BlockBounds to satisfy clippy ([1a5ef2b](https://github.com/Meridiona/meridian/commit/1a5ef2b1115ff6e87c7559d57af295b5cadc8c2a))
* **etl:** remove LIMIT cap on ocr_samples and elements_samples extraction ([b716085](https://github.com/Meridiona/meridian/commit/b716085627f379fbc9a1f5bf3bac8e1bd90413cd))
* **etl:** resolve 0s duration on single-frame sessions (Option D) ([317ceb2](https://github.com/Meridiona/meridian/commit/317ceb211333d9c1a4b73f48e33c285da6498194))
* **etl:** restore LIMIT 10 on elements_samples extraction ([1032b65](https://github.com/Meridiona/meridian/commit/1032b65d91a282f16f694ebf88dd5328c047584d))
* **evals:** correct model label phi-4-4bit → Qwen3.5-9B-OptiQ-4bit + TODO for /info endpoint ([925b1e8](https://github.com/Meridiona/meridian/commit/925b1e896305208efc1ae8ed20bbcd78cffa300e)), closes [#1](https://github.com/Meridiona/meridian/issues/1) [#1](https://github.com/Meridiona/meridian/issues/1)
* **evals:** force-flush root eval.run span before obs_shutdown ([25af90d](https://github.com/Meridiona/meridian/commit/25af90d662157ec28feab9533af753d5a3d90674))
* **evals:** wire Dev B candidates into render_seeds.py — normalise id/task_key ([6a39692](https://github.com/Meridiona/meridian/commit/6a39692ee5822a5d064b125a86e1ba17ab35cd0a))
* **fmt:** apply cargo fmt to intelligence modules ([ca49a35](https://github.com/Meridiona/meridian/commit/ca49a357ea6fdf57856d215a0f084d2965e94638))
* **hooks:** block edits to committed migration files in pre-commit ([4555892](https://github.com/Meridiona/meridian/commit/455589217af772ef62e0498949d4533e9328cdd7))
* **install:** correct permissions walkthrough — Microphone pane has no '+' button ([cfdaf34](https://github.com/Meridiona/meridian/commit/cfdaf3418bfaee5baca861e387f75e8b3b725e1f))
* **install:** don't abort MLX installer when MERIDIAN_OO_AUTH is unset ([3255f62](https://github.com/Meridiona/meridian/commit/3255f6224f3f109f1b9a6283b18e8a93834326fe))
* **install:** find latest OO release that actually has darwin binaries ([46ac6a7](https://github.com/Meridiona/meridian/commit/46ac6a7c5e8d7056ed03342f07dadeac5eb9a522))
* **install:** improve MLX server startup process and timeout handling ([1748b5c](https://github.com/Meridiona/meridian/commit/1748b5c20ffb71271999e56e716c9aa16036f3ff))
* **install:** make meridian stop actually stop daemons under KeepAlive ([eec56da](https://github.com/Meridiona/meridian/commit/eec56da1896e2b4353e0e46bb5e27b848efb48d5))
* **install:** pin screenpipe to 0.3.350 via npm ([62c9973](https://github.com/Meridiona/meridian/commit/62c99735e284f852f2005882ec67e28c03a76f30))
* **install:** prompt for OO email+password separately instead of raw base64 ([27ef7c4](https://github.com/Meridiona/meridian/commit/27ef7c4e375dbdfe9f00f3cf0275f854580d136a))
* **install:** prompt for OO email+password separately instead of raw base64 ([fbf08b6](https://github.com/Meridiona/meridian/commit/fbf08b66bd91befb47989e06f14272226a031696))
* **install:** reduce MLX readiness probe to 60s; don't block on first-run download ([0e2eebc](https://github.com/Meridiona/meridian/commit/0e2eebc0afe3a50526ac1720c8673a9f3763abc4))
* **install:** run pip install -e . so agents package is importable after setup ([43184b3](https://github.com/Meridiona/meridian/commit/43184b336367c3ead0ceb31a3409ce4d45aca8ce))
* **install:** seed services/.hermes/ before env collection + switch screenpipe to npm ([3d07cb6](https://github.com/Meridiona/meridian/commit/3d07cb6a4bfb456cd5ac0c7a32fadd20c025a08b))
* **install:** stream MLX log on first run; fast probe when model is cached ([32b4992](https://github.com/Meridiona/meridian/commit/32b49926ba53eaebc06dff7a515329215f2b4f79))
* **install:** sync shared env vars and pin screenpipe to ~0.3 ([06ed055](https://github.com/Meridiona/meridian/commit/06ed0558f85d642a7891134e2dd1b08c89c19cc3))
* **install:** update screenpipe install hint from brew to npm ([f3f3a80](https://github.com/Meridiona/meridian/commit/f3f3a8061027c1c8894bb20cb43ce1712816f6b9))
* **install:** update screenpipe install hint in install-screenpipe-daemon.sh ([d46a3fd](https://github.com/Meridiona/meridian/commit/d46a3fd04024631936805acc1e9243890757441c))
* **install:** wait for bootout before bootstrap in daemon/ui/screenpipe installers ([7ec084f](https://github.com/Meridiona/meridian/commit/7ec084f9a3d045441c7db508ca3057a09172c148))
* **install:** wait for launchd bootout before MLX bootstrap (avoid EIO) ([7b007bb](https://github.com/Meridiona/meridian/commit/7b007bb1f98c1975b6871e933f9154b5f197d9eb))
* **intelligence:** address code review findings across Rust backend ([8bdcf8f](https://github.com/Meridiona/meridian/commit/8bdcf8fe53455e7c9dba7a887add9f60a7bdef5d))
* **intelligence:** category settler skips historical sessions on first run ([ea03cc1](https://github.com/Meridiona/meridian/commit/ea03cc1e203f7137193b8e16c2783571ac7ec4da))
* **intelligence:** classify all new browser sessions immediately, skip backlog ([f7f251f](https://github.com/Meridiona/meridian/commit/f7f251fdf0996e62c07b2687cff9df7c2ace28da))
* **intelligence:** classify all new browser sessions immediately, skip backlog ([22999f3](https://github.com/Meridiona/meridian/commit/22999f326139d834c8ee66f8161e0571a5e7cb67))
* **intelligence:** embed skill inline and disable toolsets to fix parse failures ([47ba668](https://github.com/Meridiona/meridian/commit/47ba66894e8a45e0cdbb71a1efb912a90dac0853))
* **intelligence:** fix MLX deprecated API, fast-fail on server exit, add psutil+mlx-lm deps ([fedcf5b](https://github.com/Meridiona/meridian/commit/fedcf5bfeaf9cd77ae8917d7e2297673bb4f9c3a))
* **intelligence:** harden Chrome category settler and add tests ([4682d12](https://github.com/Meridiona/meridian/commit/4682d124475b2c86a1e62a7792f00ae79f97a4db))
* **intelligence:** harden Chrome category settler and add tests ([de23ea3](https://github.com/Meridiona/meridian/commit/de23ea3130ec53b05e2b45b08a2df12a9160eae1))
* **intelligence:** log Foundation Models skip as warn instead of silently swallowing ([633a85e](https://github.com/Meridiona/meridian/commit/633a85e407e6dc14819cc2df8b8d9a41883ae851))
* **intelligence:** log Foundation Models skip as warn instead of silently swallowing ([2e05d7b](https://github.com/Meridiona/meridian/commit/2e05d7b445ec750df0075a86d8128acad437637e))
* **intelligence:** promote run_task_linker python stderr from debug to info ([cdd858d](https://github.com/Meridiona/meridian/commit/cdd858dcbb9879407eb9dc74b1535548c2089ab0))
* **intelligence:** skip backlog, lower browser session threshold to 5s ([46a0e66](https://github.com/Meridiona/meridian/commit/46a0e66b0b2b4d45c66bebf866bf6e2027cd6bff))
* **intelligence:** skip backlog, lower browser session threshold to 5s ([a24e220](https://github.com/Meridiona/meridian/commit/a24e22056ec0cae9be808fd3fe9c1bdee33b4cd0))
* **intelligence:** strip non-Latin characters from FM prompts ([17cf377](https://github.com/Meridiona/meridian/commit/17cf377cc32531c3086b145292c2c336fa93d603))
* **intelligence:** strip non-Latin characters from FM prompts ([ff00332](https://github.com/Meridiona/meridian/commit/ff003323fb31d4fb8895acea4cd4ec1ba8d7d28f))
* **intelligence:** write sentinel on permanent FM failures to prevent endless retry ([3480092](https://github.com/Meridiona/meridian/commit/34800929cb62eeeb34ce3dd44c77466bb7faf9af))
* **intelligence:** write sentinel on permanent FM failures to prevent endless retry ([395ea8d](https://github.com/Meridiona/meridian/commit/395ea8dceab5f8e66f127f6180b163ef7b0b032f))
* **launchd:** apply hardened bootout/enable pattern to install-daemon.sh ([53aeb70](https://github.com/Meridiona/meridian/commit/53aeb7021e62db9e7dcb2fa1b94349f6c4ca5f4f))
* **launchd:** harden all daemon installers against bootstrap failures ([a0c123e](https://github.com/Meridiona/meridian/commit/a0c123ebde52d996ca0e896b45bacb1267fbce87))
* **llm-selector:** detect Ollama installed models and install mlx-lm on Apple Silicon ([745b5b9](https://github.com/Meridiona/meridian/commit/745b5b90410bf82c1e490b9cb395f778ed230ec8))
* **llm-selector:** read LLM_BUDGET_PCT from config when budget_pct not passed ([c7051f3](https://github.com/Meridiona/meridian/commit/c7051f3723542b83d12a43e4d81d9b798efc64f5))
* **llm-selector:** stabilise model selection by adjusting headroom for running managed server ([030bc7c](https://github.com/Meridiona/meridian/commit/030bc7c3f22ede3efdb0d66743d1d3afcb3927a6))
* **llm-selector:** stabilise model selection by adjusting headroom for running managed server ([e689a16](https://github.com/Meridiona/meridian/commit/e689a1672d774dec2983ee1542fb28c92e40c344))
* **llm-selector:** use LM Studio /api/v0/models to detect loaded models ([00b55ec](https://github.com/Meridiona/meridian/commit/00b55ec074806c49ca132cfa31fd383de2f18274))
* **llm-selector:** wait for managed server to die before starting replacement ([890a0f4](https://github.com/Meridiona/meridian/commit/890a0f4d722ce0952a86b1a4669fe04799e49fa2))
* **llm-selector:** wait for managed server to die before starting replacement ([da4629b](https://github.com/Meridiona/meridian/commit/da4629b04981e1f58ab6305a883545310139014f))
* **mcp:** replace better-sqlite3 with sql.js to eliminate native module version errors ([b13b907](https://github.com/Meridiona/meridian/commit/b13b9071cd25c9387a088b942a245d0c04dcb96a))
* **mcp:** use local node path instead of npx until package is published to npm ([3f99326](https://github.com/Meridiona/meridian/commit/3f993266ca1b9ce2d0bb2e0dbbc218e1bbf68ff2))
* **migrations:** add category_method column to app_sessions table ([55b063b](https://github.com/Meridiona/meridian/commit/55b063b48e055da4020f61107123c845320be0e2))
* **migrations:** add category_method column to app_sessions table ([9e06cfe](https://github.com/Meridiona/meridian/commit/9e06cfe9cbd5682fbdc358dce6cbd860e5fe3571))
* **migrations:** drop and recreate pm_task_embeddings around pm_tasks swap in 022 ([5fd5fa2](https://github.com/Meridiona/meridian/commit/5fd5fa2a44a3c3d6f9a371b1705fc1990758328c))
* **mlx-server:** remove ProcessType Background; extend readiness probe to 600s ([785ab1f](https://github.com/Meridiona/meridian/commit/785ab1fe255f7090ba9b3bcab1be7cd119d45ce8))
* **observability:** disable OTel in dev to prevent Map size exhaustion ([05c7f8a](https://github.com/Meridiona/meridian/commit/05c7f8a488d910151119c58a87b3963ae2bf3006))
* **observability:** flush OTel spans before short-lived subprocess exits ([588beef](https://github.com/Meridiona/meridian/commit/588beefe0e0a11a79c5a4c1970859a7fef9729b6))
* **observability:** log to stdout so daemon.log / 'meridian logs' captures it ([110869e](https://github.com/Meridiona/meridian/commit/110869ea3f4d1767252addb49364edb02daec05f))
* **observability:** update OpenObserve installation to use fixed version v0.11.0 ([34e43b5](https://github.com/Meridiona/meridian/commit/34e43b5d02160eff7855e5008ea2deec3be1dfca))
* **observability:** use .instrument() to parent run_task_linking under poll_tick ([47fd674](https://github.com/Meridiona/meridian/commit/47fd674621671169291fd6f472df59ba4254f0c6))
* **openobserve:** cap memory, default log level to warn, fix crash-loop ([80f4bec](https://github.com/Meridiona/meridian/commit/80f4becfa75f75e23f489b140abfb756a54c37c1))
* **pm-tasks:** exclude subtasks and normalize status categories ([5a62a0c](https://github.com/Meridiona/meridian/commit/5a62a0c2ef1225d2a590491e9239c74843325bf1))
* **release:** build on macOS 26 SDK, complete FFI stub, fetch tags ([7503155](https://github.com/Meridiona/meridian/commit/75031551c8e17e60590908fd94c00c5b26addb8f))
* **release:** publish arch package by path, not npm owner/repo shorthand ([cafa1aa](https://github.com/Meridiona/meridian/commit/cafa1aa1e9b10fb78b770e903af550127e1010c1))
* **screenpipe:** enhance get_frame_full_texts to fallback on accessibility_text when full_text is NULL ([c6d2158](https://github.com/Meridiona/meridian/commit/c6d2158ae9465caa8137daea8f37494fc2e5faac))
* **screenpipe:** wrap npm script in /bin/sh for launchd compatibility ([3ff2d1a](https://github.com/Meridiona/meridian/commit/3ff2d1a1a9c3830590ddbc559d8bd0cb45dc63f7))
* **scripts:** chain coding-agent-indexer daemon and Claude hook into install scripts ([42fbac3](https://github.com/Meridiona/meridian/commit/42fbac3167e42dd2df71eece54f39114a303bbf4))
* **services:** address Python code review findings and clean up pipeline ([09f4866](https://github.com/Meridiona/meridian/commit/09f4866392cdeea2614257f91b3c0ef3541ea2b3))
* **services:** tighten Python dependency version bounds ([3ee9971](https://github.com/Meridiona/meridian/commit/3ee997107bdcddfc36af49efc003ead6e9a14b1f))
* **settings:** number stepper allows free typing without clamping mid-edit ([d43c8b0](https://github.com/Meridiona/meridian/commit/d43c8b0c5eddf21be007416f782a708bd7ad7ee8))
* **settings:** number stepper allows free typing without clamping mid-edit ([e021d98](https://github.com/Meridiona/meridian/commit/e021d9866423322b1b18674ad562397194ff50b2))
* **tagger:** correct Ollama Cloud base URL to https://ollama.com/v1 ([ada1c06](https://github.com/Meridiona/meridian/commit/ada1c060b8cc605f046558cc6e9f2c342f93496c))
* **tagger:** replace ocr_samples with session_text throughout Python services ([135de33](https://github.com/Meridiona/meridian/commit/135de334f51015c17a3cff99ccf031b11eece4bd))
* **tagger:** use Ollama Cloud env vars for Stage 3 LLM config ([d07d3c9](https://github.com/Meridiona/meridian/commit/d07d3c944b5b6fc4705f2d0393e4a0d7bb3e5739))
* **task-classifier:** clarify session_type logic and confidence scoring ranges ([48bf82d](https://github.com/Meridiona/meridian/commit/48bf82d02f3a6e6d820333947aabe41fef2f89b2))
* **task-classifier:** correct example session_type from overhead to unknown ([ea43d88](https://github.com/Meridiona/meridian/commit/ea43d888414327081749b5f8c052d52657beb8ba))
* **task-classifier:** intermediate sessions should link to task if work-related to prior context ([cab5185](https://github.com/Meridiona/meridian/commit/cab5185fb98b6d4202529d37900fd86a235f8f03))
* **task-classifier:** raise confidence for clear work signals without task mapping (0.6-0.8) ([58cd4dd](https://github.com/Meridiona/meridian/commit/58cd4dde42c593e6d27057e2b61ccd6794de5ebb))
* **task-classifier:** require work alignment for high confidence, not just task key visibility ([6042878](https://github.com/Meridiona/meridian/commit/60428787ae34eed1636ab6e28cc32c0cf536317c))
* **task-classifier:** standardize session_type naming to 'untracked' throughout ([3494bb3](https://github.com/Meridiona/meridian/commit/3494bb33a58a00e69c1bafadc09b6984f5007a79))
* **task-linker:** add reasoning and session_type to classification debug logs ([486074b](https://github.com/Meridiona/meridian/commit/486074baee76aad9750d9c7e4ffe2eed03ccb6c7))
* **task-linker:** reduce batch limit to 1 session per daemon tick ([8b66354](https://github.com/Meridiona/meridian/commit/8b663543f78205bcd0ff14ea13279c0bc2e0463c))
* **task-linker:** remove unused fetch_sessions_by_ids ([06baaaf](https://github.com/Meridiona/meridian/commit/06baaaf652bb960b34de72ff5c4fef41430febbe))
* **task-linker:** resolve post-merge protocol mismatch and import errors ([db98586](https://github.com/Meridiona/meridian/commit/db98586bee3c28d797129fbceef9060e6df07477))
* **tests:** add accessibility_text column to screenpipe mock schema ([134e78c](https://github.com/Meridiona/meridian/commit/134e78cd005c7b2db4523e5491b006a962dc09f2))
* **tests:** add missing accessibility_text column to test frames schema ([6a34d02](https://github.com/Meridiona/meridian/commit/6a34d02a029ce7340c78597f777f1e9d9fbceda0))
* **tests:** enhance parse_category tests with alias handling and verbose prose fallback ([a1c963b](https://github.com/Meridiona/meridian/commit/a1c963ba1712176aba035a8f91647cbe2eacbae0))
* **tests:** mark LLM smoke tests as ignored to unblock pre-push ([a6f8016](https://github.com/Meridiona/meridian/commit/a6f80161c263d952b8ff4f56c8cf06244264f632))
* **tests:** remove stale stage-flag tests and fix MCP summary regex ([6b7b6fc](https://github.com/Meridiona/meridian/commit/6b7b6fc2f768bb93d2ce2f9bc172ea61519652f0))
* **tests:** serialise LLM smoke tests to prevent parallel timeout ([eb6315a](https://github.com/Meridiona/meridian/commit/eb6315a5f8df5320a8fd390067369c088782bb5c))
* **tests:** serialize config env-var tests with a mutex ([0d87772](https://github.com/Meridiona/meridian/commit/0d87772008bbd549edc140207ab98c75827970d2))
* **timeline:** guard NaN in CSS position calculations ([817da6a](https://github.com/Meridiona/meridian/commit/817da6abcfd47fefe7bbae47140771f9e5c139ed))
* **ui:** correct meridiona typo to Meridiona in app metadata ([331f456](https://github.com/Meridiona/meridian/commit/331f45646673f9abbebc5157f9c230dbf45482b1))
* **ui:** guard NaN in tooltip left from focus event clientX ([ab68ad8](https://github.com/Meridiona/meridian/commit/ab68ad869ac0513286cbf68207c130de41b15a45))
* **ui:** make All Time label black for readability ([2e20dce](https://github.com/Meridiona/meridian/commit/2e20dcec7e677cc4b2e9d1efcdd00fe9f9aacae2))
* **ui:** pin exact package versions, fix typescript@5.8.2 (5.8.0 was never published) ([4946f7b](https://github.com/Meridiona/meridian/commit/4946f7b4629f0adfb5d9b90c633d5176a6708df1))
* **ui:** resolve hydration mismatch in DayTimeline + quiet terminal logs ([eb1b8e8](https://github.com/Meridiona/meridian/commit/eb1b8e8fc171d25fb86ba2fe812d29f39902fb13))
* **ui:** restore correct page.tsx and remove stale ocr_samples refs ([5e2b2ce](https://github.com/Meridiona/meridian/commit/5e2b2ce6be868ce17a7cff93ee62a680619fc346))
* **ui:** switch Instrument Serif to Google Fonts CDN link ([05181ca](https://github.com/Meridiona/meridian/commit/05181cabf6071ce91a7df19b621db63416da1926))
* **ui:** update TaskBadge pipeline display for hermes methods ([ee705c7](https://github.com/Meridiona/meridian/commit/ee705c771dc9efc54e3edeef8ce9b59c776b7269))
* update parameters in generate_category method to avoid unused variable warnings ([aa44c5a](https://github.com/Meridiona/meridian/commit/aa44c5ae6b1b324d4beffe5824a58d0ef84fa9c0))


### Features

* **a11y:** add ax-sidecar to inject VS Code terminal content into screenpipe ([75827a3](https://github.com/Meridiona/meridian/commit/75827a3749beebddf4af03476f8c9746d09bc548))
* add daemon entrypoint, config, and lib crate split ([dca4751](https://github.com/Meridiona/meridian/commit/dca475150164262b01d7a7904d30f2e334eeb500))
* add PM update workflow and session summaries ([c53dc79](https://github.com/Meridiona/meridian/commit/c53dc793cde1d41fa0ccd6d4207738cd5b1ad77e))
* **agents:** add Jira update agent with slot-based scheduler ([26e343b](https://github.com/Meridiona/meridian/commit/26e343bd0c60a2be9d316168bb27bf730d4bac58))
* **agents:** implement dynamic LLM selection and download logic based on machine specs ([24976f8](https://github.com/Meridiona/meridian/commit/24976f828918a665c9b9920992192a03beb355af))
* **agents:** implement dynamic LLM selection for Apple Silicon and update configuration ([0a24667](https://github.com/Meridiona/meridian/commit/0a24667f7b1ebb4d5c6673dcb0f0f0c535ea4148))
* **ax-sidecar:** add Codex session support + tag assistant role per agent ([997fd84](https://github.com/Meridiona/meridian/commit/997fd84c182886ca4b1a31711b6541c5f97f7f39))
* **ax-sidecar:** capture Claude Code session transcripts into screenpipe ([cf8757e](https://github.com/Meridiona/meridian/commit/cf8757eb78e6308edf26c1ef53d78719375d6e93))
* **ax-sidecar:** distinguish Claude windows via session title + uuid in window_name ([66e0413](https://github.com/Meridiona/meridian/commit/66e0413ff7b47c151193038849bb851f3e7723ba))
* **ax-sidecar:** pin host app per session, anchored to session start ([e36fee3](https://github.com/Meridiona/meridian/commit/e36fee3b76fcfa03185ef35294d196c5c6c3551d))
* **ax-sidecar:** resolve app_name from screenpipe instead of hardcoding it ([902d9fc](https://github.com/Meridiona/meridian/commit/902d9fcb750be4e7aabb8e19ec5a97f0520c8413))
* **backfill:** add standalone backfill scripts for categories and task links ([02b823c](https://github.com/Meridiona/meridian/commit/02b823c7d684227304a4552bd0370a10402b8b3c))
* **classifier:** log run smoke_20260528T162251 + declare failure_class schema ([f655b96](https://github.com/Meridiona/meridian/commit/f655b9638e588282f5c741116621ea59d8683194))
* **classifier:** log run smoke_20260528T180202 — SESSION_TEXT_CAP=0 experiment ([13dd636](https://github.com/Meridiona/meridian/commit/13dd6368353c4e5e34b837f6c83cf165eddddc4d))
* **coding-agent:** port indexer + summariser into the Rust daemon ([f5da20f](https://github.com/Meridiona/meridian/commit/f5da20fb37a74e133755fef0decbc2fa671ab6c0))
* **config:** multi-provider PM config architecture ([db29400](https://github.com/Meridiona/meridian/commit/db29400106862ac957fae72c51a57e4280e9d3d3))
* **daemon:** restore sequential run_task_linking for non-MLX backends ([ca4bdf7](https://github.com/Meridiona/meridian/commit/ca4bdf7520d91b9b470bc6d785f78eae736b1324))
* **db:** add screenpipe read layer and meridian write schema ([73d9183](https://github.com/Meridiona/meridian/commit/73d9183c5b3aa15502a71bde434e2a49f8ec03e2))
* **dist:** npm distribution like screenpipe (@meridiona/meridian) ([5fa278f](https://github.com/Meridiona/meridian/commit/5fa278f42f2cc28b2a5060eeba4e7b11b2242d60))
* **etl:** add block context extractor and ETL runner ([75f8783](https://github.com/Meridiona/meridian/commit/75f87830cd0093add56a39ce6d15de314239f731))
* **etl:** add structured debug tracing across gap detection, block ops, and context extraction ([a93faac](https://github.com/Meridiona/meridian/commit/a93faacf64fc711827ccb59eab3d81a18f9dcaf3))
* **etl:** categorize sessions inline during ETL, store in app_sessions ([e678541](https://github.com/Meridiona/meridian/commit/e678541ac87c286105908ed48ceac7e8cb65eb80))
* **etl:** filter OCR noise from session_text at storage time ([f3e04f2](https://github.com/Meridiona/meridian/commit/f3e04f2104074308046ba4447d120dfebdefcbdb))
* **etl:** gap detection — separate gaps table, system_sleep vs user_idle ([dbf9dc9](https://github.com/Meridiona/meridian/commit/dbf9dc9934d7ed285b8b1457c0bb28ee9a2c739d))
* **etl:** infer Antigravity from Terminal OCR, fix browser domain grouping ([4ef62c0](https://github.com/Meridiona/meridian/commit/4ef62c0c13e799966eb58412bc33a1cb2d7f75a5))
* **etl:** Option C — refine session end using ui_events click/key timestamps ([693a454](https://github.com/Meridiona/meridian/commit/693a454b3f6acc2d6d6784d540d5857839690d19))
* **etl:** split browser sessions by domain, add cat_smoke diagnostic binary ([6b25314](https://github.com/Meridiona/meridian/commit/6b253146d7c85c7de26c470fafc4cf9e7c2d50c8))
* **etl:** split VS Code sessions by project and dedup terminal keystrokes ([9586209](https://github.com/Meridiona/meridian/commit/9586209ed5a76e161fda610ff35ef32251399f8c))
* **eval-feedback:** read local results JSON instead of querying OpenObserve ([712bcd2](https://github.com/Meridiona/meridian/commit/712bcd2e92d02f9e1fceb005f8ec1e1e06d708df))
* **eval:** dynamic LLM selector — /info endpoint + auto-discovery + --model flag ([0f2b87c](https://github.com/Meridiona/meridian/commit/0f2b87c5e17cb25a85cca8a7a03d3a3e9b959d36)), closes [task#1](https://github.com/task/issues/1) [task#1](https://github.com/task/issues/1)
* **evals:** --config flag for versioned experiment manifests ([ff238c8](https://github.com/Meridiona/meridian/commit/ff238c8875d9ea3031d2cfc0d3d4295f83b09381))
* **evals:** add Dev A 35-session golden seed dataset ([a2c10f5](https://github.com/Meridiona/meridian/commit/a2c10f5aba3408f35f2b2bd8c2d7659c48fa3f58))
* **evals:** add Dev A candidates with 5 real KAN tickets + 2 decoys ([63a38ca](https://github.com/Meridiona/meridian/commit/63a38caf44065cc09f34b2cd3c7f98b89e9a2e08))
* **evals:** add Dev B generic persona dataset — sessions 1-15 of 35 ([3e98004](https://github.com/Meridiona/meridian/commit/3e980040ad3bc4550bb6b65478155b63d075690b))
* **evals:** add EvalStrategy abstraction for pluggable classifier configs ([bd87c42](https://github.com/Meridiona/meridian/commit/bd87c42cdd6851baea1e19e28691ad407813c59b))
* **evals:** add MLX classifier evaluation suite with DeepEval ([eb62d49](https://github.com/Meridiona/meridian/commit/eb62d491c8efd1f1d0fd38c77f484408e0b9d497))
* **evals:** add overhead + untracked tiers to b_generic dataset ([133ccb6](https://github.com/Meridiona/meridian/commit/133ccb60a072d799507460e15607c96f6976c35f))
* **evals:** add render_seeds.py + smoke_run.py for the classifier eval pipeline ([722499a](https://github.com/Meridiona/meridian/commit/722499abb18f062138c3e223b8bc7fc05097057f))
* **evals:** Dev B sessions 16-20 — PROJ-201 completion and PROJ-225 profiling ([f50e600](https://github.com/Meridiona/meridian/commit/f50e600407c6992a7039412c387980f40aaa6210)), closes [#88](https://github.com/Meridiona/meridian/issues/88) [#94](https://github.com/Meridiona/meridian/issues/94) [#frontend](https://github.com/Meridiona/meridian/issues/frontend)
* **evals:** Dev B sessions 21-25 — hard discrimination cases and PROJ-230 ([9163e5d](https://github.com/Meridiona/meridian/commit/9163e5d59064ccd8e3974868fdd63ed4df91a001)), closes [#92](https://github.com/Meridiona/meridian/issues/92) [#93](https://github.com/Meridiona/meridian/issues/93) [#92](https://github.com/Meridiona/meridian/issues/92) [#95](https://github.com/Meridiona/meridian/issues/95)
* **evals:** Dev B sessions 26-30 — bug fix, scope control, and PROJ-242 deference ([7eee871](https://github.com/Meridiona/meridian/commit/7eee87127c1cd3190ef5349a6a2e9f801bbb1d6e)), closes [#94](https://github.com/Meridiona/meridian/issues/94) [#88](https://github.com/Meridiona/meridian/issues/88) [#96](https://github.com/Meridiona/meridian/issues/96)
* **evals:** Dev B sessions 31-35 — complete generic persona dataset (35/35) ([fe926ad](https://github.com/Meridiona/meridian/commit/fe926ad3673c691f5e956e892278f13f3dfd18f3))
* **evals:** experiment config for extract_then_classify on Dev B ([d481fbb](https://github.com/Meridiona/meridian/commit/d481fbb17385e4594bcedc0b213d91b5ce0dc462))
* **evals:** ExtractThenClassifyStrategy — two-stage classification (Task [#13](https://github.com/Meridiona/meridian/issues/13)) ([54a469f](https://github.com/Meridiona/meridian/commit/54a469f96f865ae3c3cc6164ecd11e1e6204e729))
* **evals:** log eval run qwen35-9b-optiq-4bit-b_generic-baseline-20260529 ([9cf31ff](https://github.com/Meridiona/meridian/commit/9cf31ff487567eb623d701c72d006796e4477e54))
* **evals:** log run b_generic_direct_http_20260530T110547 to FEEDBACK ([b02ea61](https://github.com/Meridiona/meridian/commit/b02ea6195b6b3911c988497144a2976ae543d7d0))
* **evals:** log run b_generic_extract_then_classify_20260530T190121 to FEEDBACK ([0cfa02e](https://github.com/Meridiona/meridian/commit/0cfa02e82c52598fd87778d227fdf6b84e302d89))
* **evals:** meaningful run_id + richer OO trace coverage ([89043da](https://github.com/Meridiona/meridian/commit/89043da0e603b456e8b5bbc8d59866ab3fe63ed3))
* **evals:** nested OO traces for ExtractThenClassifyStrategy stages ([2abb880](https://github.com/Meridiona/meridian/commit/2abb880c7135ef4b32a0a6eb3cf216f336751302))
* **evals:** write canonical results JSON per eval run ([2d40c5e](https://github.com/Meridiona/meridian/commit/2d40c5e726e7ff0ec1d4248be8f351dbe87e4041))
* **install:** add --mlx flag to install.sh for persistent MLX server ([899cc5a](https://github.com/Meridiona/meridian/commit/899cc5aaf007aea3c9f5ecf8ebc33adfb23b9a7b))
* **install:** add OpenObserve as an optional prereq ([2f62758](https://github.com/Meridiona/meridian/commit/2f627585e3c1c7921de00f05efdb26245f523a4b))
* **install:** auto-start OpenObserve as a launchd LaunchAgent ([7e0fde0](https://github.com/Meridiona/meridian/commit/7e0fde0c729c10496135e3e9c5a1930ec1e091e6))
* **install:** interactive credential collection + install-package test suite ([e04ba3e](https://github.com/Meridiona/meridian/commit/e04ba3ee3d5d189cd936ceed6abc01b7c00ba977))
* **install:** manage the Next.js dashboard as a launchd LaunchAgent ([dd32acc](https://github.com/Meridiona/meridian/commit/dd32acccbd7e65892d43a4c5d9390c143179d826))
* **install:** MLX default, single repo .env, retire jira-updater ([1fc93c7](https://github.com/Meridiona/meridian/commit/1fc93c78414f50c9dab8ca3d3c0ffbd6d483c689))
* **install:** one-command install with launchd-managed screenpipe, meridian daemon, and jira-updater ([2342b46](https://github.com/Meridiona/meridian/commit/2342b4662d26e64b1e18313f6d9290aadbf260e8))
* **install:** one-command prebuilt installer (no clone, no build) ([7837381](https://github.com/Meridiona/meridian/commit/78373813614d8da68fc69ce4253e09946410a505))
* **intelligence:** add backfill binaries for session categories and task classification ([69606d9](https://github.com/Meridiona/meridian/commit/69606d9586011ce6aeb5f69aeece38743bc05758))
* **intelligence:** add development category and include session_text in FM category prompt ([f27ebac](https://github.com/Meridiona/meridian/commit/f27ebaceb8c8a9f96c7b1755a4793e5f84119087))
* **intelligence:** add dynamic local LLM selection for task classifier ([ad3fbc2](https://github.com/Meridiona/meridian/commit/ad3fbc24bbb244704d131bc94bedf97d08bd7120))
* **intelligence:** add LLM classifier pipeline with Foundation Models support ([9c53223](https://github.com/Meridiona/meridian/commit/9c53223061157835ca76b6ac13ab2acf794e25c1))
* **intelligence:** add LLM classifier pipeline with Foundation Models support ([c088e00](https://github.com/Meridiona/meridian/commit/c088e00d6a8bdd2e99d748bd192376ad7fb65971))
* **intelligence:** add pm_tasks and ticket_links migrations ([2d0b363](https://github.com/Meridiona/meridian/commit/2d0b363b69d67d35653b0462d328a99566c12c56))
* **intelligence:** add Swift Foundation Models FFI bridge ([368dbe8](https://github.com/Meridiona/meridian/commit/368dbe804a42b10cb1ac7dcbc0596000b8717351))
* **intelligence:** add Swift Foundation Models FFI bridge ([2017ec2](https://github.com/Meridiona/meridian/commit/2017ec2020714bd0ae7b6ce787ebd162e5b1ee28))
* **intelligence:** add task-key debug logging to PM provider sync ([9c421c2](https://github.com/Meridiona/meridian/commit/9c421c2f1916d25053130f7e9067ad44a75fbb05))
* **intelligence:** add tracing instrumentation to jira provider ([313a414](https://github.com/Meridiona/meridian/commit/313a41408d19503c573760b77aa2a7d7e2629151))
* **intelligence:** distinguish category confirmed vs updated in settler logs ([9e63381](https://github.com/Meridiona/meridian/commit/9e6338127b63d559cd77f1038c7c73a195abab6f))
* **intelligence:** enforce FM structured output via @Generable for category classification ([1bdf7f0](https://github.com/Meridiona/meridian/commit/1bdf7f0ab7e54a4407fcef686d2e113d35d2422d))
* **intelligence:** expand FM category classification to all apps ([acf2425](https://github.com/Meridiona/meridian/commit/acf2425cf2ad5ff9227845843df5eee6a5d05884))
* **intelligence:** Jira REST connector and provider stubs ([2d2cc94](https://github.com/Meridiona/meridian/commit/2d2cc94584cea4ca02631081fa80c99e1ebdf077))
* **intelligence:** propagate run_fm_categorization rename and add tracing spans ([ce75135](https://github.com/Meridiona/meridian/commit/ce7513585f5a884cf6bdba2bccd862d27539053f))
* **intelligence:** session activity categorizer with 86 unit tests ([c3f4d1c](https://github.com/Meridiona/meridian/commit/c3f4d1cc6a3d35fe950383afe3997040ba32e505))
* **intelligence:** wire dynamic LLM endpoint into hermes AIAgent ([7169143](https://github.com/Meridiona/meridian/commit/7169143d11cdabfd61b0325b868dcd8b25309d46))
* **intelligence:** wire into daemon, add reqwest and dotenvy ([c75a3f4](https://github.com/Meridiona/meridian/commit/c75a3f4d5c16be5d78528ff3d897f5693cd0c786))
* **launchd:** add MLX server daemon scripts ([82e9d95](https://github.com/Meridiona/meridian/commit/82e9d957a3f87e927a54102ad09116a0c9186a07))
* **llm-gate:** add process-global single-permit LLM gate ([025ee53](https://github.com/Meridiona/meridian/commit/025ee5368c2adca413f836f78ab91dd1bb447dc2))
* **llm-gate:** serialise classify + summarise MLX calls through the gate ([d4d0fc1](https://github.com/Meridiona/meridian/commit/d4d0fc126eec40da80bc5db00f700cac1f96a80d))
* **llm-selector:** add comprehensive logging and tracing to model selection ([be4a1dd](https://github.com/Meridiona/meridian/commit/be4a1ddaed28b3acab5f89d1c875197ed34a9608))
* **llm-selector:** add comprehensive logging and tracing to model selection ([4c08b70](https://github.com/Meridiona/meridian/commit/4c08b709606f45bde775162c3874c07a50d2ee53))
* **llm-selector:** auto-unload managed mlx server when external server detected ([1b47393](https://github.com/Meridiona/meridian/commit/1b47393a86cb4447c1c74a908a2779f26f4626bd))
* **logs:** split errors into a separate stream/command (daemon + MLX) ([f2b6104](https://github.com/Meridiona/meridian/commit/f2b6104fa5412463c8279fd28bd845d170e6fbf9))
* **main:** enable browser category settler after each ETL pass ([6722bda](https://github.com/Meridiona/meridian/commit/6722bda7cb97801bcba2ccae139eb21ae922fd19))
* **main:** enable browser category settler after each ETL pass ([5591021](https://github.com/Meridiona/meridian/commit/55910210be55f947f112c8193af6004a5bb096fe))
* **mcp:** add npx installer that auto-configures Claude Desktop and Cursor ([ff29db9](https://github.com/Meridiona/meridian/commit/ff29db986e75cc1148a0112644354c9a4f66a7b0))
* **mcp:** add production-ready Meridian MCP server ([cd341bd](https://github.com/Meridiona/meridian/commit/cd341bd945b9f4467c3679db7ee8cbab6804effc))
* **mcp:** install into Claude Code CLI — user and project level ([594e6c6](https://github.com/Meridiona/meridian/commit/594e6c6ea2b12fb7154dcb337797fb37956593d1))
* **mcp:** remove audio_snippets from LLM-facing tool responses ([e186aa9](https://github.com/Meridiona/meridian/commit/e186aa972b284847b703d2007423677497304dff))
* **mcp:** remove ocr/elements from queries, expose session_text ([0846e9d](https://github.com/Meridiona/meridian/commit/0846e9dbd9ab34bb80c434d1897d226dc983c159))
* **meridian-agents:** adapt db.py to 005 schema with new helpers ([4d6c080](https://github.com/Meridiona/meridian/commit/4d6c080a6abf2ce05c26537f123c6a59bad39c2d))
* **meridian-agents:** adapt db.py to 005 schema with new helpers ([40348bb](https://github.com/Meridiona/meridian/commit/40348bb0a82ddcb36a5e2b140606e88e36aa0676))
* **meridian-agents:** add llm.py — async wrapper around hermes AIAgent ([6a1f30e](https://github.com/Meridiona/meridian/commit/6a1f30e59da9597629e1d9e2fa99b58f3d250aa1))
* **meridian-agents:** add llm.py — async wrapper around hermes AIAgent ([de70eda](https://github.com/Meridiona/meridian/commit/de70edab30246257038b8bf919f80550233ae5a9))
* **meridian-agents:** config and db layers with full test suite ([13eb999](https://github.com/Meridiona/meridian/commit/13eb999ab9d9429e289f77d6aba1e120c4cb5dd8))
* **meridian-agents:** config and db layers with full test suite ([3102720](https://github.com/Meridiona/meridian/commit/3102720bfe9813944eb82aafbbca6288589386fd))
* **meridian-agents:** event-driven tagger via long-running daemon ([c8a8f68](https://github.com/Meridiona/meridian/commit/c8a8f68197b81abf822b045e58e03b0a2e897e90)), closes [hi#water-mark](https://github.com/hi/issues/water-mark)
* **meridian-agents:** event-driven tagger via long-running daemon ([210201b](https://github.com/Meridiona/meridian/commit/210201b0f5e2bea98dd1ff722cc7ab2d1eb0006e)), closes [hi#water-mark](https://github.com/hi/issues/water-mark)
* **meridian-agents:** hot-toggle stages without restarting the daemon ([07b874f](https://github.com/Meridiona/meridian/commit/07b874faea1bea01617ed263556c9f101d1c2326))
* **meridian-agents:** hot-toggle stages without restarting the daemon ([0da3d13](https://github.com/Meridiona/meridian/commit/0da3d13fac1b0597d33ac8e137459eb56b9c0131))
* **meridian-agents:** integrate synthesizer with meridian.db ([177515b](https://github.com/Meridiona/meridian/commit/177515b0888a77da14eae08348309269de40fa4e))
* **meridian-agents:** integrate synthesizer with meridian.db ([29dc01e](https://github.com/Meridiona/meridian/commit/29dc01e05002464ff0813a5306c75070ebcbf1f0))
* **meridian-agents:** parallelize synthesizer tag phase ([37d0907](https://github.com/Meridiona/meridian/commit/37d0907ca9bb46b4ecf74b779d894bfe8c0d87df))
* **meridian-agents:** parallelize synthesizer tag phase ([c34131c](https://github.com/Meridiona/meridian/commit/c34131c0af5c0f98f618146a9c5710aa72286c0d))
* **meridian-agents:** per-stage enable/disable flags ([3ffa928](https://github.com/Meridiona/meridian/commit/3ffa928475bed256fcc6e18ca73d34636b3ba840))
* **meridian-agents:** per-stage enable/disable flags ([79dc1bc](https://github.com/Meridiona/meridian/commit/79dc1bc93e5a4b3d052e31de354e182c04a91690))
* **meridian-agents:** tagger single-session inspection mode ([dfbe13d](https://github.com/Meridiona/meridian/commit/dfbe13d7559905d4acb9f2d4e1b63b78473d7dfb))
* **meridian-agents:** tagger single-session inspection mode ([cae1d9c](https://github.com/Meridiona/meridian/commit/cae1d9c011ee3134203a368724137616696cc97e))
* **meridian-agents:** tagger Stage-1 — multi-dimensional rules ([0d32ea5](https://github.com/Meridiona/meridian/commit/0d32ea5eb89f88024f916f3476459e47fa7584a5))
* **meridian-agents:** tagger Stage-1 — multi-dimensional rules ([5ddfefe](https://github.com/Meridiona/meridian/commit/5ddfefe4b82c4b63d13cc626d51745001a62721b))
* **meridian-agents:** tagger Stage-2 — embedding-based ticket matcher ([dd09775](https://github.com/Meridiona/meridian/commit/dd097756b5cf86b81b8c09b1659d4546745f625b))
* **meridian-agents:** tagger Stage-2 — embedding-based ticket matcher ([f93ad18](https://github.com/Meridiona/meridian/commit/f93ad18edccd5f1b83c6c903fd8914783b724bd0))
* **meridian-agents:** tagger Stage-2 multi-sample max-pooling ([4ac67da](https://github.com/Meridiona/meridian/commit/4ac67da821e75ab5c3e2f37fb3a2c12d09af3eb6)), closes [#10](https://github.com/Meridiona/meridian/issues/10) [#1](https://github.com/Meridiona/meridian/issues/1)
* **meridian-agents:** tagger Stage-2 multi-sample max-pooling ([e11cb63](https://github.com/Meridiona/meridian/commit/e11cb63599e0a6a88cca858086739e3a35ef67b9)), closes [#10](https://github.com/Meridiona/meridian/issues/10) [#1](https://github.com/Meridiona/meridian/issues/1)
* **meridian-agents:** tagger Stage-3 — LLM tiebreaker ([da86bf4](https://github.com/Meridiona/meridian/commit/da86bf460951c2a9cbb37af6f0a6353db61977fe))
* **meridian-agents:** tagger Stage-3 — LLM tiebreaker ([be7801a](https://github.com/Meridiona/meridian/commit/be7801a89d6d1fbc5ccc70aa5867ba1e44708f0c))
* **meridian-agents:** vendor hermes runtime so AIAgent imports clean ([211a6fe](https://github.com/Meridiona/meridian/commit/211a6fe66eb1cffe14e9bdbd8ea01451df16c603))
* **meridian-agents:** vendor hermes runtime so AIAgent imports clean ([9ae1935](https://github.com/Meridiona/meridian/commit/9ae1935fa2cd6214efdbaa5ac67bdb3c7bf038c6))
* **meridian-agents:** vendor hermes-agent v2026.5.7 in-repo ([373f7b6](https://github.com/Meridiona/meridian/commit/373f7b6c9d445f7b16a6194a8c8267ff1af28198))
* **meridian-agents:** vendor hermes-agent v2026.5.7 in-repo ([ef4816c](https://github.com/Meridiona/meridian/commit/ef4816c2e2905ea309c3065dd70eec6a9e157f87))
* **migrations:** add 004_agents.sql for meridian-agents service ([bb9f81d](https://github.com/Meridiona/meridian/commit/bb9f81d062e491b6cd28746224c057e492d578ff))
* **migrations:** add 004_agents.sql for meridian-agents service ([445756c](https://github.com/Meridiona/meridian/commit/445756c6b5595fae5b7ed726bef69dbbfbdff655))
* **migrations:** remove ocr_samples/elements_samples, add session_text ([ebdbfd7](https://github.com/Meridiona/meridian/commit/ebdbfd7bdde2b9bfa78b9c647c75cb4223ec05d4))
* **migrations:** restore 014 drop of ocr_samples/elements_samples ([a151639](https://github.com/Meridiona/meridian/commit/a151639a4b13265a462e9be61bd098c573d6464f))
* **mlx-server:** add /synthesise_worklog endpoint ([8aad1e0](https://github.com/Meridiona/meridian/commit/8aad1e0d530f5c294513c745b48ae24469cef114))
* **mlx:** add run_task_linker_mlx module for in-process inference ([e7d3152](https://github.com/Meridiona/meridian/commit/e7d315279329354a15fb387a349695995b90491d))
* **observability:** add full prompt/response text to span events for LLM debugging ([e5c6045](https://github.com/Meridiona/meridian/commit/e5c604573d31085cbb85e8c31e766f905a9be91a))
* **observability:** add llm.model/runtime/is_local to spans and log records ([50a8ba3](https://github.com/Meridiona/meridian/commit/50a8ba3b600661bc23c655830ff3d5deefa9b1f3))
* **observability:** add MERIDIAN_TRACING_DISABLED to skip OTLP exporter ([abacb4c](https://github.com/Meridiona/meridian/commit/abacb4c6083cb4ac96f599851b5546c2e55a6c8f))
* **observability:** add OTel instrumentation to run_task_linker_mlx ([844c8e6](https://github.com/Meridiona/meridian/commit/844c8e6adb049d2e23aafdd0a979672107db30b5))
* **observability:** emit info event for each trivial session in run_task_linking ([b277578](https://github.com/Meridiona/meridian/commit/b27757839ee0ffaf89ef9ae68d36d17be8f0d5a1))
* **observability:** end-to-end distributed tracing into OpenObserve ([4b4afc7](https://github.com/Meridiona/meridian/commit/4b4afc7bf99a2c9a67c3664547a6e90b885f2658))
* **observability:** end-to-end OTLP tracing via OpenObserve ([30019e8](https://github.com/Meridiona/meridian/commit/30019e8308627c39a26aa44dcc4f07b20201b683))
* **observability:** end-to-end OTLP tracing via OpenObserve ([ce783ca](https://github.com/Meridiona/meridian/commit/ce783ca6807bac9b33047c405563a74e1a7072d7))
* **observability:** enhance LLM detection and add OpenTelemetry dependencies ([3387006](https://github.com/Meridiona/meridian/commit/33870066b18b2b2f99e2688725913dfd0b382534))
* **observability:** enhance tracing and observability for task linking and ETL processes ([2f4674a](https://github.com/Meridiona/meridian/commit/2f4674ac75fcab7f825f95a41ab7d880dc858284))
* **observability:** enhance tracing configuration and context management ([c1ed6ef](https://github.com/Meridiona/meridian/commit/c1ed6ef3648f9e7b016686c8a767a13420def9e5))
* **observability:** integrate opentelemetry-appender-tracing for enhanced logging ([b01bae9](https://github.com/Meridiona/meridian/commit/b01bae92a4d63b04c1c34aa3d2d8937144c3c9bc))
* **observability:** parent run_task_linking spans under poll_tick/startup_tick ([91ae1b2](https://github.com/Meridiona/meridian/commit/91ae1b2c46b794e7488e842fa33c2fe63dcb2f73))
* **observability:** propagate Rust traceparent into MLX server classify_sessions span ([745c193](https://github.com/Meridiona/meridian/commit/745c1932562186b686e8c27f7dcefae97ac56aa4))
* **observability:** wire W3C traceparent across Rust→Python boundary and add span events ([02fe0ce](https://github.com/Meridiona/meridian/commit/02fe0ce1e3659cfcf7222723395ae1fb36fc681e))
* **pm-worklog:** add 'meridian worklog-status' command ([859c991](https://github.com/Meridiona/meridian/commit/859c991841175ca239c1dba330db2c6f5dd6db1d))
* **pm-worklog:** in-daemon Stage 4 worklog pipeline ([59e554c](https://github.com/Meridiona/meridian/commit/59e554ccfcf67d0c186d34f955250eede5b394cc))
* **pm-worklog:** wire hourly driver + CLI into the daemon ([9454f43](https://github.com/Meridiona/meridian/commit/9454f438e7c2821078ecb38db8afbe080c69902a))
* **prompts:** update SESSION_TEXT_CAP to read from environment variable for eval experiments ([95a1457](https://github.com/Meridiona/meridian/commit/95a1457a03e34d420bf3e709f5c652cb23cb86a1))
* **release:** prebuilt macOS arm64 release pipeline ([95f363a](https://github.com/Meridiona/meridian/commit/95f363a1a56a50fd4218a8fab85e806b8d06df72))
* replace ax sidecar with coding agent indexer ([f59bfbf](https://github.com/Meridiona/meridian/commit/f59bfbfaae615b1900004fc3881d52b3daacdacc))
* **rust:** replace subprocess with HTTP call to persistent MLX server ([7ac4c03](https://github.com/Meridiona/meridian/commit/7ac4c031ca3dc67457700ccf282cc6e0b5575202))
* **scripts:** add setup-services.sh for co-dev onboarding ([e316c83](https://github.com/Meridiona/meridian/commit/e316c83552e1179400e7b34302c15639cdd926f3))
* **server:** add POST /classify_sessions endpoint to MLX server ([0d13857](https://github.com/Meridiona/meridian/commit/0d13857bd195c9765ddf722fd2952ab309f8e46b))
* **session_text:** replace ocr/elements samples with full_text union ([7ffa003](https://github.com/Meridiona/meridian/commit/7ffa0036da17ceeb2761501fb24fa9606996da1b))
* **session-categorizer:** add comprehensive tracing events similar to extractor ([01d4d7f](https://github.com/Meridiona/meridian/commit/01d4d7f2bb1842675c7eff53f8a8d7dd2505fe63))
* **session-categorizer:** add detailed reasoning breakdown to logs ([9eab3a2](https://github.com/Meridiona/meridian/commit/9eab3a2f4208413c196fa5a98a5330806b46dcb8))
* **session-categorizer:** add reasoning explanation to categorization logs ([f595fbc](https://github.com/Meridiona/meridian/commit/f595fbcef30171e95740fbf30fd6fd867eceb791))
* **session-categorizer:** add score breakdown for debugging reasoning ([144db85](https://github.com/Meridiona/meridian/commit/144db8572f5fbf65bff2bbaa53f92f970309a21f))
* **settings:** Apple-style feel on Radix UI controls ([706364e](https://github.com/Meridiona/meridian/commit/706364e9294bd851729c68334a1c3e0bbdef05bc))
* **settings:** Apple-style feel on Radix UI controls ([9ff3e33](https://github.com/Meridiona/meridian/commit/9ff3e33978bc6d3bce2cdba69439eb10591c50e1))
* **settings:** Apple-style select and stepper controls ([e2bcce1](https://github.com/Meridiona/meridian/commit/e2bcce13ed9040b383c6cb97c5dbf3c521411b69))
* **settings:** embed settings as dashboard SPA view with correct theme ([f88524e](https://github.com/Meridiona/meridian/commit/f88524ebb9024ec129aee85d8e76812aeca5c0ef))
* **settings:** replace custom controls with Radix UI primitives ([70026bb](https://github.com/Meridiona/meridian/commit/70026bb20ef0e0950c4f96835f8ae8dc74de4c50))
* **settings:** replace custom controls with Radix UI primitives ([b4b0c5b](https://github.com/Meridiona/meridian/commit/b4b0c5b4adc7b8d2f006101c7bdcf259ffbc1892))
* **settings:** runtime config UI backed by ~/.meridian/settings.json ([483c98f](https://github.com/Meridiona/meridian/commit/483c98f4e68157c4f26f009f84097e3492fa620f))
* **settler:** add why field to FM category response ([f56b3ae](https://github.com/Meridiona/meridian/commit/f56b3ae39cae56b8d9a9e3ef00389639a1b37332))
* **settler:** disable OCR in category prompt, add category_smoke binary ([c6d1d63](https://github.com/Meridiona/meridian/commit/c6d1d63841ea7aa822e39d37559ef59bda2e42bf))
* **settler:** disable OCR in category prompt, add category_smoke binary ([bf225b4](https://github.com/Meridiona/meridian/commit/bf225b4c9c89e4fd327a44577923851f2b2b97d5))
* **settler:** store FM category explanation in DB and retry on unsupported language error ([a52f57a](https://github.com/Meridiona/meridian/commit/a52f57ad285b0f2484ca042fcf4b321ec189c268))
* **skills:** add eval-feedback Claude Code skill to maintain FEEDBACK.json ([7a692f0](https://github.com/Meridiona/meridian/commit/7a692f0025e725cff5fc7ab4ba00ceb508ed30f6))
* **task-linker:** add startup preflight check for classification stack ([b95d324](https://github.com/Meridiona/meridian/commit/b95d3242fe6dc9f036908d1cd2ec9d82c25e706c))
* **task-linker:** store hermes reasoning in ticket_links ([11afaf1](https://github.com/Meridiona/meridian/commit/11afaf1faee75f946490f9cee85dd3a2b181e0b6))
* **ui/api:** add today, tasks, queue-review, and week route handlers ([2d48a15](https://github.com/Meridiona/meridian/commit/2d48a150792c2f561f204717a50354c91eb53f1e))
* **ui:** add design tokens, dark mode, and theme context ([3cdb4ca](https://github.com/Meridiona/meridian/commit/3cdb4caa7f24d86250f288fbcf3268a5a74483a6))
* **ui:** add navigation shell and dashboard page ([e0c90d4](https://github.com/Meridiona/meridian/commit/e0c90d4fae0feb66bb2bf3e473a8b9932c66e6fe))
* **ui:** add Next.js 15 activity dashboard ([6a5ecf7](https://github.com/Meridiona/meridian/commit/6a5ecf744c7295a5d9096548f3bdf838f630a035))
* **ui:** add shared atom components ([1338be8](https://github.com/Meridiona/meridian/commit/1338be86f71c772f939efe3ebc5ecab194d35e6b))
* **ui:** add task classification badges with pipeline tooltip to all views ([d568bd3](https://github.com/Meridiona/meridian/commit/d568bd3655ed612ba376ec3913c3bb121832d23f))
* **ui:** add Today, Tasks, Queue, Sessions, and Week views ([f3e9faf](https://github.com/Meridiona/meridian/commit/f3e9faf8dcbdd4c12e38735908fb54a938070a4e))
* **ui:** category-aware dashboard — badges, timeline colors, breakdown chart ([f91fc6f](https://github.com/Meridiona/meridian/commit/f91fc6f9e43c343143e8127bc7897f6e0c042359))
* **ui:** load more pagination on sessions page ([15d0fd7](https://github.com/Meridiona/meridian/commit/15d0fd7f384537c5d52f1d591013f12a6ddf6f47))
* **ui:** redesign dashboard with new design system and enhanced views ([7b28633](https://github.com/Meridiona/meridian/commit/7b2863373d553ef785a62fcc505f154dec69c8f6))
* **ui:** remove ocr/elements types and components, clean up session card ([f776eca](https://github.com/Meridiona/meridian/commit/f776ecaa8d7459e7b4ce6a9392f9fe89a9102c39))
* **ui:** render gaps table — idle/sleep blocks on timeline + away stats ([f2f39cb](https://github.com/Meridiona/meridian/commit/f2f39cb3b1e17efdb59b4f0b4680066bec057cb6)), closes [#D4D1CB](https://github.com/Meridiona/meridian/issues/D4D1CB) [#C8C6C1](https://github.com/Meridiona/meridian/issues/C8C6C1)
* **ui:** replace broken Recharts tooltip with inline hover highlight on donut ([19b0322](https://github.com/Meridiona/meridian/commit/19b032267cb21d471d832cf0cb8b27761e0e2a5f))
* **ui:** session detail page at /sessions/[id] ([441d801](https://github.com/Meridiona/meridian/commit/441d801522b2b4b8b3c17a2d2a3eb877660f59b4))
* **ui:** session detail page at /sessions/[id] ([b0ac6c0](https://github.com/Meridiona/meridian/commit/b0ac6c0571d66bb96bdd198d2a148b087cd96c89))
* **ui:** show all session data — OCR, accessibility elements, audio, signals ([67c7464](https://github.com/Meridiona/meridian/commit/67c74646b3a5a7b67e9bd73167ff45d39672dc26))
* **ui:** show coding-agent time in the totals strip ([6db8557](https://github.com/Meridiona/meridian/commit/6db8557120b7bd2062dd3a2a2dfac34c73ce5ac0))
* **ui:** show Jira task on every SessionCard ([86d5a8c](https://github.com/Meridiona/meridian/commit/86d5a8ce8be5819befd166779e1cc1593a1b9803))
* **ui:** Today's Tickets tile on the dashboard ([2c8afa4](https://github.com/Meridiona/meridian/commit/2c8afa41fc85e2e0d1c4c057f8ab84f58e80de65)), closes [#E8E6E1](https://github.com/Meridiona/meridian/issues/E8E6E1)
* **ui:** Today's Tickets tile on the dashboard ([e1926fb](https://github.com/Meridiona/meridian/commit/e1926fb3f4f82c3cda59058f4b2676c70e9db7c8)), closes [#E8E6E1](https://github.com/Meridiona/meridian/issues/E8E6E1)


### Performance Improvements

* **etl:** deduplicate session data at SQL level to reduce LLM noise ([b7d5039](https://github.com/Meridiona/meridian/commit/b7d5039ad5b60bb50321f3bc0723da781bdb275e)), closes [hi#frequency](https://github.com/hi/issues/frequency)
* **etl:** eliminate redundant SELECTs and cap audio snippets ([9b6f1b4](https://github.com/Meridiona/meridian/commit/9b6f1b434904efd78eaaf615bb29900ad6fa106c))
* **etl:** increase BATCH_SIZE from 500 to 2000 for faster initial migration ([81df44d](https://github.com/Meridiona/meridian/commit/81df44d42edbba299e65e8892aab5d9a92d9f1f6))
* **skill:** teach eval-feedback to read FEEDBACK.json lean via jq ([fdec347](https://github.com/Meridiona/meridian/commit/fdec34747bca3d4f23d114f067e9645f29ab3382))


### BREAKING CHANGES

* services/meridian-agents/src/meridian_agents/db.py and
its tests still reference summary_json/activity_kind on app_sessions —
they will fail until updated in a follow-up commit. Intentional: this
migration step lands in isolation per the agreed sequencing.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
* services/meridian-agents/src/meridian_agents/db.py and
its tests still reference summary_json/activity_kind on app_sessions —
they will fail until updated in a follow-up commit. Intentional: this
migration step lands in isolation per the agreed sequencing.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>

# [0.1.0](https://github.com/Meridiona/meridian/compare/v0.0.0...v0.1.0) (2026-06-01)


### Bug Fixes

* **release:** build on macOS 26 SDK, complete FFI stub, fetch tags ([7503155](https://github.com/Meridiona/meridian/commit/75031551c8e17e60590908fd94c00c5b26addb8f))


### Features

* **dist:** npm distribution like screenpipe (@meridiona/meridian) ([5fa278f](https://github.com/Meridiona/meridian/commit/5fa278f42f2cc28b2a5060eeba4e7b11b2242d60))
