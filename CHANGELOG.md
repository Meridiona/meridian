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
* **tray:** use meridiana-mark logo instead of generic placeholder ([1a2fac4](https://github.com/Meridiona/meridian/commit/1a2fac41cc7435235fd860caa2c1e314bf4ea90f))

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
* **ui:** correct Meridiana typo to Meridiona in app metadata ([331f456](https://github.com/Meridiona/meridian/commit/331f45646673f9abbebc5157f9c230dbf45482b1))
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
* **ui:** correct Meridiana typo to Meridiona in app metadata ([331f456](https://github.com/Meridiona/meridian/commit/331f45646673f9abbebc5157f9c230dbf45482b1))
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
