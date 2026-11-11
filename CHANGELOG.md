# Changelog

All notable changes to this project are documented in this file.

## [0.35.0](https://github.com/oisin-ee/dogsled/compare/v0.34.2...v0.35.0) (2026-11-11)


### Features

* **checkout:** keep a worker's repository at a standard path on peers ([b988f48](https://github.com/oisin-ee/dogsled/commit/b988f484aeb7980f6ebc249beeee573eaee5d128))
* collect agent batch with repository transfer, catalogue recovery and load-race fixes ([6cb3a7b](https://github.com/oisin-ee/dogsled/commit/6cb3a7bc9d159c4c1372b292886dda28aac5b8cd))
* **repo:** open a repository on any host ([c2fd182](https://github.com/oisin-ee/dogsled/commit/c2fd182406f63aba0ca5c892ae9c6b35f867747f))
* **service:** log per-phase observation timing once a minute ([99a1b91](https://github.com/oisin-ee/dogsled/commit/99a1b91bf836e1e61d530ed3e324926be321ec01))
* **transfer:** apply received changes to working files ([e3512cf](https://github.com/oisin-ee/dogsled/commit/e3512cf546dcbade9c7050780ee23ca459845429))
* **transfer:** capture and import a pinned source ([89e7044](https://github.com/oisin-ee/dogsled/commit/89e704465ab3cd14190616277c4c7c89a1e8ba56))
* **transfer:** capture and import a pinned source ([e30c58f](https://github.com/oisin-ee/dogsled/commit/e30c58f51e010c4796c4c51d66e57abbc0b09ebd))
* **transfer:** deliver transfers to another host ([bba9ca9](https://github.com/oisin-ee/dogsled/commit/bba9ca90b997777b91aa0175a658acf47c75629f))
* **transfer:** send, list, show, and drop transfers ([4ebd024](https://github.com/oisin-ee/dogsled/commit/4ebd024a1205a40dd0da09c9e1ca9846c101b254))


### Bug Fixes

* **activity:** bound registration refusal memory and skip invalid panes ([63bf732](https://github.com/oisin-ee/dogsled/commit/63bf732f6036139d2cf0574e63d93c559706c50c))
* **activity:** register Pi live detail when verifiable, without console output ([f443a7d](https://github.com/oisin-ee/dogsled/commit/f443a7de94011a4c9f8c38050955a6decc9a78d6))
* **activity:** verify producer registration against a post-receipt sample ([332ffe8](https://github.com/oisin-ee/dogsled/commit/332ffe86adc8c6fdbd3198742050d9e82af9383c))
* finish the activity-startup batch (header notices, test watcher leak, load-stable peer fixtures) ([67c8949](https://github.com/oisin-ee/dogsled/commit/67c89497d96e474f89b0f7c8d5668b8d22838fc0))
* **inbox:** hide requests from agents that have closed ([59b9e1e](https://github.com/oisin-ee/dogsled/commit/59b9e1eb84871ae4fe2cc1ec1559509062deacaa))
* **launch:** back off refused-job release retries and log changes only ([ff3cc69](https://github.com/oisin-ee/dogsled/commit/ff3cc69e2efc2c2f693b44b72f216d92088ba596))
* **observation:** log dropped IPC clients ([#321](https://github.com/oisin-ee/dogsled/issues/321)) ([a479b93](https://github.com/oisin-ee/dogsled/commit/a479b9337e57a3bfa38a22fad6a2dcfe01eb1d34))
* **observation:** measure one collection in the boundary test ([737a119](https://github.com/oisin-ee/dogsled/commit/737a1195e2dc471307847d227bbdb4654c56801d))
* **pi:** preserve terminal observation errors ([#320](https://github.com/oisin-ee/dogsled/issues/320)) ([a02b1f8](https://github.com/oisin-ee/dogsled/commit/a02b1f8427392062f987bda3a165d9331d21d365))
* **pi:** register live activity once the session is verifiable and stop console output ([6da106e](https://github.com/oisin-ee/dogsled/commit/6da106e42d668201c8edcfa41b70b2c5d296f2cc))
* **pi:** stop test watchers after failed assertions ([983ab8c](https://github.com/oisin-ee/dogsled/commit/983ab8c90e8e1194438e17aee967e67880acf6be))
* **portal:** publish the repository catalogue on the portal stream so the picker recovers ([01688d2](https://github.com/oisin-ee/dogsled/commit/01688d29b6d092ec22e46b5ada4a033e47de8afe))
* **shell:** let deadline cleanup reap the command leader on macOS ([560683c](https://github.com/oisin-ee/dogsled/commit/560683c640294b3d36d00bb0eaf81ed58d193265))
* **sidebar:** exit when the terminal is lost and reap test sidebars ([a31ed64](https://github.com/oisin-ee/dogsled/commit/a31ed6493dffa1120621a7e83f86a89abf36bec0))
* **sidebar:** watch the terminal on stdin, since Darwin reports POLLNVAL for /dev/tty ([6248a12](https://github.com/oisin-ee/dogsled/commit/6248a1269474fb5785ce32c9e5a05ece0def5de5))
* **todo:** fence delta sessions from older extensions and validate deltas on read ([abbdc33](https://github.com/oisin-ee/dogsled/commit/abbdc33f4c2222b7b8b34b130bf01d004478928f))
* **todo:** save task changes as deltas instead of full snapshots ([c4ed5a3](https://github.com/oisin-ee/dogsled/commit/c4ed5a34164db0624693b5164415d751527da226))
* **transfer:** record the source commit and reuse Git isolation ([2ee24e2](https://github.com/oisin-ee/dogsled/commit/2ee24e23a5912705b9c0d7616419374bf2618045))
* **wait:** report a deadline reached before the target was resolved ([b223d58](https://github.com/oisin-ee/dogsled/commit/b223d58ac5506515dc49d3948d8ee68794d3bdfa))
* **web:** drain refused request bodies so refusals are not reset ([#318](https://github.com/oisin-ee/dogsled/issues/318)) ([454b286](https://github.com/oisin-ee/dogsled/commit/454b286a18985480314eba7849e52abbada971d0))
* **web:** keep catalogue notices out of the New header ([f86bbe4](https://github.com/oisin-ee/dogsled/commit/f86bbe41e4a3d7083f3fb4716540af1a1f579867))
* **web:** keep the retained catalogue when a fetch fails without repositories ([cda0892](https://github.com/oisin-ee/dogsled/commit/cda0892eda01994a2f1b8bdd37ec6023bff8455b))


### Performance Improvements

* **activity:** read only appended transcript bytes for conversation detail ([f25aed4](https://github.com/oisin-ee/dogsled/commit/f25aed420e2b18bd62564ced5530f4623b775a44))

## [0.34.2](https://github.com/oisin-ee/dogsled/compare/v0.34.1...v0.34.2) (2026-11-09)


### Bug Fixes

* **activity:** let producers recover registration without stealing ([#317](https://github.com/oisin-ee/dogsled/issues/317)) ([66c5774](https://github.com/oisin-ee/dogsled/commit/66c577460e728713e55fdd152526c59a132f9b8c))
* **release:** reuse the PR's CI proof instead of rerunning the suite ([aab621c](https://github.com/oisin-ee/dogsled/commit/aab621c650b1c05f904b52ea48f892b475f72df5))
* **release:** reuse the PR's CI proof instead of rerunning the suite ([ce78863](https://github.com/oisin-ee/dogsled/commit/ce78863b1b97bad4d9807313d31354a02fea365c))

## [0.34.1](https://github.com/oisin-ee/dogsled/compare/v0.34.0...v0.34.1) (2026-11-09)


### Bug Fixes

* **portal:** keep resource holds out of the Workers tab ([a13b8aa](https://github.com/oisin-ee/dogsled/commit/a13b8aa4777fdeb3c3d5cacfcc307696dcb5fc71))
* **portal:** keep resource holds out of the Workers tab ([834ecbb](https://github.com/oisin-ee/dogsled/commit/834ecbbd736840092ba52757dc0a66c9dfd409b8))
* **web:** keep the local activity producer routes stable ([cbaf87d](https://github.com/oisin-ee/dogsled/commit/cbaf87d485c32e1191310e6bd94f1fb4b5687c5f))
* **web:** keep the local activity producer routes stable ([b63db6e](https://github.com/oisin-ee/dogsled/commit/b63db6e1bf2d8ef11936644b825489003dde36a8))

## [0.34.0](https://github.com/oisin-ee/dogsled/compare/v0.33.0...v0.34.0) (2026-11-08)


### Features

* **activity:** carry reasoning and tool detail on the wire ([2c85a42](https://github.com/oisin-ee/dogsled/commit/2c85a42a1ad1abd63db0e2545af01078d58bd0a6))
* **activity:** carry reasoning and tool detail on the wire ([f553fb8](https://github.com/oisin-ee/dogsled/commit/f553fb88abf0fdbaa290116b1314fe99d1448760))
* **files:** capture bounded typed Git status ([3435682](https://github.com/oisin-ee/dogsled/commit/343568258e531fa5ff99153084d68747093f0047))
* **leads:** GitHub catalogue, leads on any host, portal on every host ([83c65d7](https://github.com/oisin-ee/dogsled/commit/83c65d71396d24388cebb7ec9e1836965bcd9643))
* **leads:** start leads on the roomiest fleet host ([11f3cb2](https://github.com/oisin-ee/dogsled/commit/11f3cb27745a4d06f4b912c5cc44d56f8f0656d9))
* **portal:** address agents across fleet hosts ([ce9f63b](https://github.com/oisin-ee/dogsled/commit/ce9f63bc5732f9aba5ca8f619ecc796b515e6702))
* **portal:** move lead workers into a Workers tab ([73dea6c](https://github.com/oisin-ee/dogsled/commit/73dea6c2d4a70ac82084964c31722a5c31776c42))
* **portal:** move lead workers into a Workers tab ([c484cae](https://github.com/oisin-ee/dogsled/commit/c484cae3af6aa158b666dbafd2f7f256d1c80ac4))
* **portal:** serve files as an independent agent detail part ([3560f36](https://github.com/oisin-ee/dogsled/commit/3560f364284124558ba2f71f73c0177d3102da4d))
* **portal:** show checkout changes in a Files tab ([fac7a08](https://github.com/oisin-ee/dogsled/commit/fac7a080f40c9275c8801ebf12f2e8251b3cf232))
* **portal:** show checkout changes in Files tab ([7d0b93e](https://github.com/oisin-ee/dogsled/commit/7d0b93ece29883adc60e47671ee38215bfa019a3))
* **repos:** publish GitHub catalogue for the New picker ([28f71c0](https://github.com/oisin-ee/dogsled/commit/28f71c04a4b37097828751e3c3be9cebbf4e7ad3))
* **web:** render reasoning and tools in one conversation stream ([d55a567](https://github.com/oisin-ee/dogsled/commit/d55a5675fadc27b7ca0f899643c003a9c3ef4bfe))
* **web:** render reasoning and tools in one conversation stream ([4d1c504](https://github.com/oisin-ee/dogsled/commit/4d1c504d34501e4ce2ebf55f8754d28535624719))
* **web:** share held remote agent streams ([9fa7520](https://github.com/oisin-ee/dogsled/commit/9fa752034675ae1abed31b0f3a00b496f49486ca))


### Bug Fixes

* **agents:** publish independent detail part revisions ([f8221b2](https://github.com/oisin-ee/dogsled/commit/f8221b29327c85fd84929780bcd4348d85db95b6))
* **board:** leave hold rows without an owner host ([f84b922](https://github.com/oisin-ee/dogsled/commit/f84b9225aad753ae0b8e18103ef69d7c2d3feccf))
* **cleanup:** attribute detached agent tool processes to panes ([a6bfc95](https://github.com/oisin-ee/dogsled/commit/a6bfc9569bbacec36279ef78c623fcaefdbe456c))
* **cleanup:** attribute detached agent tool processes to panes ([7b2add5](https://github.com/oisin-ee/dogsled/commit/7b2add5e182bd7b0060cbc3b90c4131df387cc98))
* **files:** pin Git status diagnostics to the C locale ([126c728](https://github.com/oisin-ee/dogsled/commit/126c72831d602e6c0c61472569508c02047be472))
* **holds:** read validity rows and their owners together ([29f5b91](https://github.com/oisin-ee/dogsled/commit/29f5b91c1498bb33f5adcc11d128456663382bc6))
* **observation:** a missing long state path reads as serve not running ([ca8dfdf](https://github.com/oisin-ee/dogsled/commit/ca8dfdf8925234937f36b7323eb9e1b05a0fea50))
* **observation:** an unlisted pane is not found even with a captured detail ([bc12f7b](https://github.com/oisin-ee/dogsled/commit/bc12f7bc8dee0ce6a6a5519acf5eda1e89137e17))
* **observation:** deliver pane detail directly from workers ([5e28a8c](https://github.com/oisin-ee/dogsled/commit/5e28a8c72272f8d8a91f86c4691a43f4bca6d551))
* **observation:** deliver pane detail directly from workers ([8316294](https://github.com/oisin-ee/dogsled/commit/83162943c6f3e7de9bc77281ce67148e1ce85b20))
* **observation:** publish revisions only for content changes ([1b91542](https://github.com/oisin-ee/dogsled/commit/1b91542ec2394b4972b8e8c2adc5e1b119b32e04))
* **portal:** distinguish unsupported files from peer failures ([d59ce3b](https://github.com/oisin-ee/dogsled/commit/d59ce3bef8a4743667add24abad217d79f04b3a2))
* **portal:** let accordions fill their column ([16d1458](https://github.com/oisin-ee/dogsled/commit/16d14587cdef20ceb683700d10d128b6ac07aaaa))
* **portal:** refetch only changed agent view parts ([2c160b4](https://github.com/oisin-ee/dogsled/commit/2c160b4f703c8d8fead23d77df3d01d1962b0d87))
* **portal:** refuse unrelated prompts to blocked agents ([601a48f](https://github.com/oisin-ee/dogsled/commit/601a48f0e202a86ec43b05d8d6feb28a1d3323d3))
* **preservation:** hash and read snapshot files in one Git process each ([03d8d08](https://github.com/oisin-ee/dogsled/commit/03d8d08ddad7b932ffb2b113f84e131fbaf6202d))
* **preservation:** hash and read snapshot files in one Git process each ([4455067](https://github.com/oisin-ee/dogsled/commit/4455067c8ab514669abdf8f0950d9065864ca6e2))
* **release:** gate and tag only the commit the run is for ([8af7b39](https://github.com/oisin-ee/dogsled/commit/8af7b3919b2a3471e58953c96bb8409a77d96af8))
* **release:** settle every outstanding release, not only the push that merged it ([c0a85bf](https://github.com/oisin-ee/dogsled/commit/c0a85bf6c444534530a4e736c1130c14bb8e8988))
* **release:** settle every outstanding release, not only the push that merged it ([e659c8c](https://github.com/oisin-ee/dogsled/commit/e659c8c114d8169e41cd9b58b1f841ce618ea6fb))
* **sidebar:** keep observation gaps off the terminal it draws on ([f16b8a8](https://github.com/oisin-ee/dogsled/commit/f16b8a8420afa43b988cba177314ff78ca31c997))
* **web:** drop the review list's unused import ([841fee4](https://github.com/oisin-ee/dogsled/commit/841fee40da7fc98c2b930a864bc628bf5772ec27))
* **web:** keep agent view loading until its first snapshot ([1f5498e](https://github.com/oisin-ee/dogsled/commit/1f5498eca1dfe93be7492ea06d10cdb185d68a06))
* **web:** make reviews and questions follow inbox revisions ([fe23d64](https://github.com/oisin-ee/dogsled/commit/fe23d64e2867ddc661ed60712a39897fd87d4e31))
* **web:** read agent parts once and retain activity stream ([698841e](https://github.com/oisin-ee/dogsled/commit/698841ed11a8b2610bbf10f87151a571c9042922))
* **web:** refetch only changed agent view parts ([b620dcc](https://github.com/oisin-ee/dogsled/commit/b620dcc7a8e9cd7072359ccbe11bf8d6817ac4f9))

## [0.33.0](https://github.com/oisin-ee/dogsled/compare/v0.32.0...v0.33.0) (2026-11-05)


### Features

* **activity:** publish Pi events and follow session-safe live data ([a4fd315](https://github.com/oisin-ee/dogsled/commit/a4fd315635ffa6e8471a49f4f7044c1dfe212d49))
* **leads:** start, keep, and close top-level Pi leads from CLI, MCP, and portal ([8a1f2dc](https://github.com/oisin-ee/dogsled/commit/8a1f2dcfb8f08ede14a888880dc6759c71e39bb5))
* **leads:** persist ownerless records and exclude admission demand ([5f3ff63](https://github.com/oisin-ee/dogsled/commit/5f3ff63d30c81cdd1b951be552c29ab4c93b829f))
* **leads:** persist ownerless records and exclude admission demand ([9249a9f](https://github.com/oisin-ee/dogsled/commit/9249a9f96f639c6c41cf4f83cb50ddc49f34a7f3))
* **observation:** centralize session-qualified agent details ([f1fea4d](https://github.com/oisin-ee/dogsled/commit/f1fea4dd7a6b4352e16b057a542156a71b240f05))
* **observation:** centralize session-qualified agent details ([262c8cf](https://github.com/oisin-ee/dogsled/commit/262c8cf89d0acd70351fa3981f1f64ab04e51079))
* **observation:** collect local facts in serve ([fc77bc0](https://github.com/oisin-ee/dogsled/commit/fc77bc0ca7431d2ac85345c05ee4f730f10bbadc))
* **observation:** collect local facts in serve ([69d7d9e](https://github.com/oisin-ee/dogsled/commit/69d7d9e604405a7796ebdec127ef6f017e3d924e))
* **observation:** cut hooks, lanes and placement to the publication ([cf98fd1](https://github.com/oisin-ee/dogsled/commit/cf98fd14eb5d99f9d3e3422b224d365d04b0f95a))
* **observation:** exchange host-owned peer facts in serve ([d6d2612](https://github.com/oisin-ee/dogsled/commit/d6d26127f6cdfa4172313bfa470bef7ea49afcce))
* **observation:** exchange host-owned peer facts in serve ([be1e688](https://github.com/oisin-ee/dogsled/commit/be1e68819486cb2fb404de0927e884b92abdb430))
* **observation:** publish immutable revisions over a local socket ([f57f52b](https://github.com/oisin-ee/dogsled/commit/f57f52bddbcb6ec2952cbd8a745a223d68f33749))
* **observation:** publish immutable revisions over a local socket ([8bc8ef2](https://github.com/oisin-ee/dogsled/commit/8bc8ef2ff2e7281467ece0a7b3e57df6e6f8bc6f))
* **observation:** publish shared reader projections ([b9a11cf](https://github.com/oisin-ee/dogsled/commit/b9a11cfda1d8a40f61814da7995c7cc5530fb0a5))
* **observation:** publish shared reader projections ([a469f9a](https://github.com/oisin-ee/dogsled/commit/a469f9ae07300ad9beb1129d5c760f1327beb78c))
* **observation:** read CLI and MCP views from publications ([ac00db5](https://github.com/oisin-ee/dogsled/commit/ac00db573280caceca61fb527b2b6a560535ba98))
* **observation:** read CLI and MCP views from publications ([06a6ab6](https://github.com/oisin-ee/dogsled/commit/06a6ab6ff68c9f50060ebd0f3533620debb63231))
* **observation:** serve one portal revision ([a10fd76](https://github.com/oisin-ee/dogsled/commit/a10fd7636f37db7b97e6beb7295b239ae7b9e3d7))
* **observation:** wake collection from local changes ([6f2ce50](https://github.com/oisin-ee/dogsled/commit/6f2ce50c41b02337a0bb6e15149b9eb9c8b36f6e))
* **observation:** wake collection from local changes ([a6b3a98](https://github.com/oisin-ee/dogsled/commit/a6b3a983b4aa961aba67ec0ca32a69c2f5ca3077))
* **observation:** serve-owned live activity authority ([569f00b](https://github.com/oisin-ee/dogsled/commit/569f00bcdf75c5b63116420d7a79d0dd678a3c15))
* **pi:** follow one publication subscription per session ([2628488](https://github.com/oisin-ee/dogsled/commit/26284880a5280c936d3985be741c7251fb1dbeac))
* **portal:** attributed conversation history for the timeline ([36e10b6](https://github.com/oisin-ee/dogsled/commit/36e10b6b8f43da1fd216522eb031f7b87ef0f2a8))
* **portal:** render the conversation message timeline ([0d8c0e1](https://github.com/oisin-ee/dogsled/commit/0d8c0e1727daba40fe87b57c2a706c9a7e7f42be))
* **repo:** configure repository list and CLI commands ([6835289](https://github.com/oisin-ee/dogsled/commit/68352890fb043a2fee533cc8dc5a4753f99f62a8))
* **repo:** configure repository list and CLI commands ([a0a938a](https://github.com/oisin-ee/dogsled/commit/a0a938ad710a170297db2788eb18c536cb76b52d))
* **sidebar:** render subscribed publications without observation ([a3a7976](https://github.com/oisin-ee/dogsled/commit/a3a7976f72033467f580dbcff103896ebc4b1702))


### Bug Fixes

* **close:** honor the operation-specific native close budget ([1b3c185](https://github.com/oisin-ee/dogsled/commit/1b3c1853245605a2f26035f14bb4bf3986ab1bc3))
* **close:** retain step errors after the command budget expires ([a6f7251](https://github.com/oisin-ee/dogsled/commit/a6f725124d8b6316511499665e71e275f4ef8259))
* **done:** release stale signal guards on newly claimed input ([cecb563](https://github.com/oisin-ee/dogsled/commit/cecb563cf624055867893d6fa50e36f54159cfd2))
* **observation:** collect todos apart from host capture ([40ae5a1](https://github.com/oisin-ee/dogsled/commit/40ae5a1d1d75ef543435809334650aad52751092))
* **observation:** gate branch checks on fresh host facts ([2e4c8e6](https://github.com/oisin-ee/dogsled/commit/2e4c8e6959daf68006b0b760168e8f0e03b9a459))
* **observation:** prune expired reservations without rewriting live rows ([b40f905](https://github.com/oisin-ee/dogsled/commit/b40f90574c0d9580b50324d13b30288c165a1b77))
* **observation:** prune reservations without replacing committed state ([5f32616](https://github.com/oisin-ee/dogsled/commit/5f32616f09968f40830196407a16419311a6c885))
* **observation:** retry native exports after legacy fallback ([3109822](https://github.com/oisin-ee/dogsled/commit/3109822976f1f86ae2dff539369ae97385200982))
* **observation:** keep a final detail publication on macOS ([00a9763](https://github.com/oisin-ee/dogsled/commit/00a9763460fe86181ee02ebcf2f7196f6c42fab6))
* **observation:** publish an unreadable hold as a gap ([639c583](https://github.com/oisin-ee/dogsled/commit/639c58336c135f5c698f8cb52816e96d1c486cf2))
* **pi:** retry activity registration after session binding changes ([7985ad6](https://github.com/oisin-ee/dogsled/commit/7985ad6f6c260377ed99d70652c04365c33462ca))
* **quota:** only serve's quota lane collects; the CLI reads storage ([cfe8725](https://github.com/oisin-ee/dogsled/commit/cfe8725b9a75338f4dd8e6b44249384c9d9e292c))
* **service:** log gap diagnostics before publishing ([ecbb2a2](https://github.com/oisin-ee/dogsled/commit/ecbb2a223e7a1e15e5612f93d5ecd95d7600b787))
* **shell:** strip repository-local GIT_* variables at the spawn point ([514a8d1](https://github.com/oisin-ee/dogsled/commit/514a8d10e3ee5714a48fad0ae443f74db88c04f5))
* **skill:** limit worker roles to research and writer ([e193bdf](https://github.com/oisin-ee/dogsled/commit/e193bdfc6d70a4f8ba698535296f5e99ec43a745))
* **worktree:** normalize local identity before pure equality ([e630277](https://github.com/oisin-ee/dogsled/commit/e63027786909063a0d79ab6ca7a4406487c77689))
* **worktree:** normalize local identity before pure equality ([8acea38](https://github.com/oisin-ee/dogsled/commit/8acea38abe3a404480891b0cf8a553f39de18d92))

## [0.32.0](https://github.com/oisin-ee/dogsled/compare/v0.31.0...v0.32.0) (2026-11-04)


### Features

* **web:** redesign the portal as a dark agent console ([31b61ee](https://github.com/oisin-ee/dogsled/commit/31b61ee25d549fc3b97a50bfed12a5e55c65c77d))


### Bug Fixes

* **done:** remove handoff.lock ([fbdcdba](https://github.com/oisin-ee/dogsled/commit/fbdcdbaa9d8d2bdeb127a880095c30b84d9674d8))
* **done:** remove handoff.lock so the children lane never waits on a close ([201289c](https://github.com/oisin-ee/dogsled/commit/201289cf0f7a2d48b4e9bf49f9cc8161cf234e7a))
* **release:** grant release-please its documented permissions and state the commit-subject rule ([a798771](https://github.com/oisin-ee/dogsled/commit/a798771368c94cb4a13b8d334630476dedd02a74))
* **release:** select the dogsled lockfile version ([f2a406b](https://github.com/oisin-ee/dogsled/commit/f2a406b3ab1091144a4dce24c013e098805fd2d1))
* **service:** children lane imports handoffs without taking handoff.lock ([4a75fc2](https://github.com/oisin-ee/dogsled/commit/4a75fc2f25c2771b9768597dab0f9fbf318512db))
* **service:** children lane runs handoff only when its lock is free ([43da9e4](https://github.com/oisin-ee/dogsled/commit/43da9e431616a7d2140be29fa1825f73e3f183d1))
* **upgrade:** keep running services alive when a newer writer's handover fails ([#259](https://github.com/oisin-ee/dogsled/issues/259)) ([996fda2](https://github.com/oisin-ee/dogsled/commit/996fda2bedfd4533f7497e92a700f383ac1b335e))
* **upgrade:** remove the executable handover ([0512826](https://github.com/oisin-ee/dogsled/commit/0512826e054f42358aef762a7a69277fdc49b495))
* **upgrade:** remove the executable handover ([121a072](https://github.com/oisin-ee/dogsled/commit/121a072471cc841991a3fc9e731ab26d5cf0876a))
* **work:** keep a released completion time instead of failing admission ([b8bc70f](https://github.com/oisin-ee/dogsled/commit/b8bc70f1565f4f2d5d0622058268a844767ffd2e))
* **work:** keep a released completion time instead of failing admission ([d6e8040](https://github.com/oisin-ee/dogsled/commit/d6e804080856502e9933a679266498933b21a2ea))

## 0.31.0 - 2026-11-03

- Settle finished and refused jobs without permanent resource alarms. Keep released replay evidence until normal expiry, record agent-pane owners, and show actionable cleanup commands.
- Remote launch errors show the host and the typed refusal reason, including a missing execution directory, instead of reporting an SSH failure.
- Probe samples and cleanup pass history are typed `state.db` tables, `samples` and `host_report`. Dogsled no longer writes `samples.jsonl` or `sweeps.jsonl`, and no state record is a JSON file.
- Items and queued tickets change in one SQLite write transaction. The `items.lock` and `queue.lock` files are gone, so a close no longer waits on a file lock to record an item. Kernel locks remain only where they guard work outside `state.db`: panes, checkouts, peer hosts, the quota service, and the capacity publication protocol.
- A write to `state.db` that starts inside another write on the same thread fails at once. Before, it waited for itself until its deadline.
- Running commands and services switch to the newest valid recorded Dogsled executable without a manual restart. Live MCP sessions keep their connection and pending request. Missing or invalid executables no longer block a compatible store. Debug builds do not register in the default state directory without an explicit override.
- `state.db` is now schema version 4. A host with an older `state.db` starts again with an empty store. Let queued work finish before you upgrade a host. Delete the old `samples.jsonl`, `sweeps.jsonl`, `items.lock`, and `queue.lock` files after you upgrade.

## 0.30.0 - 2026-11-02

- Store launch receipts, progress, releases, seeds, and continuation receipts in typed SQLite tables.
- Web questions, reviews, attachment metadata, inboxes, outboxes, boards, sidebar preferences, and todo publications use typed SQLite tables. Pi publishes todo snapshots through the CLI. Credential files and agent-readable image artifacts stay as files.
- Store preparation claims, receipts, grants, expectations, and retirements in typed SQLite tables.
- Store jobs, request validity, cancellations, placement selections, reservations, and the validity clock in typed SQLite tables.
- Store host snapshots, handoffs, cleanup, custody, watchdog, quota, and preservation state in typed SQLite tables.
- Status and prompt context show failing service lanes on each host, including when the failure started and how often it repeated. Service errors and handoff warnings print only when they change or recover.
- Admissions move from one JSON file per request into typed tables in `state.db`: one `admissions` row per request, with child tables for job arguments, declined placements, custody acknowledgements, and reviews. Each field Dogsled reads is its own column. Peer protocol messages, such as commit inputs, receipts, and release certificates, are kept as the exact bytes exchanged, because their digests and replay depend on them.
- `dogsled unit settlements`, which every Pi session runs every 10 seconds, reads only the caller's unsettled units, through an index. Before, it read every admission file several times per call and reread the whole queue for each failed or cancelled ticket, so about 11 sessions kept about 11 cores busy.
- `state.db` is now schema version 2. A host with an older `state.db` starts again with an empty store. Admission files and older store contents are not read. Let queued work finish before you upgrade a host.
- Items and queued tickets are typed tables too, with each field in its own column.

## 0.29.3 - 2026-11-02

- `dogsled close` resumes an interrupted preservation close after completed teardown, final capture, or normalization without repeating completed effects. A matching origin request can resume a requestless close for the same launch. Other identity changes still refuse the retry. Unknown teardown outcomes and interrupted normalization still retain the checkout.
- Automatic cleanup removes only recorded checkouts and branches of closed children. Checkout identity must match the recorded Git directories and generation. Worktrees and branches that Dogsled did not create stay intact, even when clean and pushed. Explicit `dogsled prune` keeps its existing behavior.

## 0.29.2 - 2026-11-02

- The web portal builds its controls from the official Dioxus components and runs no hand-written scripts. Before, it used hand-written markup and `document::eval` scripts. Timers, conversation scrolling, image upload, and the marking of annotated passages are now Rust.
- The web portal has no style sheet of its own. Colors, fonts, and the page frame come from the Dioxus components theme, so the portal follows the system light or dark setting. Before, it used a fixed Catppuccin palette.
- The conversation is no longer a box with a fixed height. The page of the selected agent scrolls and follows the newest turn.

## 0.29.1 - 2026-10-30

- Admission works again when one committed unit cannot be queued. Before, a unit request saved without request validity, for example by an older `dogsled mcp`, made the service's recovery step fail on every tick, so no request was admitted on that host. Now the service logs that request and holds it alone.
- Remote jobs that need a checkout start again. Before, a job reservation took its hold without publishing the request's validity, so the following `host checkout` refused it as unprovable on every remote host.
- A session's board, sidebar, and banner show only the resource holds that the session owns. Before, every hold without an owner pane appeared in every session as a blocked card. `dogsled board --all` and `dogsled status` still show every hold.
- Every preparation claim must name its request. Dogsled no longer reads requestless claims or reports the `legacy_preparation` hold; status reports such a record as `unreadable`.

## 0.29.0 - 2026-10-29

- Each host keeps its items and its queue in one SQLite database, `state.db`, in its state directory. A change writes only the rows that changed, in one transaction. Before, each change rewrote all of `items.json` or `queue.json`, which stalled tickets once those files grew to several megabytes.
- The new store starts empty. It does not read `items.json`, `queue.json`, or `queue-format.json`. Open items and queued tickets from 0.28.1 are not carried over. Let queued work finish before you upgrade a host.
- Dogsled expires released terminal lifecycle records. It keeps output for 24 hours and removes terminal metadata only after 30 days. Explicit prune no longer deletes job JSON at the output deadline.
- Ordinary and agent reference releases use version 2 certificates, with an explicit variant for no binding. Version 1 release contracts are unchanged.
- Recorded ordinary handoffs stay readable after their closure.
- Cancelled requests that cannot be released no longer block the queue. Before, the service retried every one of them before each admission pass, so new work could wait for hours.
  - A cancellation with no saved preparation identity is held. Its release stays pending. `dogsled status` and the board show it as `cancellation_held`, with `dogsled retire-preparation <request-id>` as the command that releases it. The service retries it only when its admission, preparation, or retirement records change.
  - Admission runs first in each service pass. Cancellation repair runs after it, outside the admission lock, and tries at most 16 releases per pass. It saves all changed tickets in one queue write.

## 0.28.1 - 2026-10-29

- The npm packages publish again. The 0.28.0 release built its binaries and GitHub release, but its npm job stopped before it published anything, so 0.28.0 is not on npm. `actions/setup-node` started pnpm caching on its own, because `package.json` names pnpm, and proto had not installed pnpm yet.

## 0.28.0 - 2026-10-29

- `dogsled stop <id>` ends a pending job that never started and prevents `dogsled exec` from running it later.
- `@oisin-ee/pi-dogsled` includes the `todo` tool. Install it alone; do not install `@oisin-ee/pi-todo` next to it. Existing sessions keep their todo lists, because the stored entries do not change.
- `@oisin-ee/oisinpi` no longer includes pi-todo. It still loads pi-prewalk, pi-tiers, and pi-guardrails.
- Dogsled and the todo tool call each other directly. The seed-path, plan-approval, and work-display events are gone. `oisin.tasks.acquire.v1` stays for extensions outside pi-dogsled, such as pi-prewalk.
- The workspace includes pi-todo, pi-prewalk, pi-tiers, pi-guardrails, and oisinpi, with their Rust crates and shared toolchains. All packages release together with the Dogsled version.
- The Dogsled unit contract types come from todo-core. The wire JSON does not change.
- A new worktree runs the `setup` commands of its own `dogsled.toml`, from its base revision. Before, setup came from the host's main checkout, which can be out of date. A base without `dogsled.toml` runs no setup, and an invalid one fails like a failed setup command.
- Lifecycle and recovery: `docs/lifecycle.md` describes the states and recovery protocols below.
  - `dogsled status`, the board, and MCP list each held resource with its owner, the reason, and the command that releases it.
  - An owner or orphan close saves dirty and deleted work in a pinned, verified Git snapshot before it removes the checkout. A failed teardown keeps the checkout and shows the blocker. Snapshots can be inspected and restored.
  - `retire-preparation <request-id>` ends future use of one historical preparation claim. It never deletes a checkout and never settles a task.
  - Placement saves its capacity grant and result, and `place --resume <request-id>` replays a request without new capacity, setup, or pane. Cancellation is ordered under admission, and an unreachable host leaves its release pending.
  - Non-job requests carry the same validity record as jobs. A finished time cannot change once set.
  - A repository lock fences worktree removal against job startup and preparation.
  - A child event whose subject is gone is refused and logged, not sent to the owner pane.

## 0.27.0 - 2026-10-28

- `dogsled claim <item> [--revision N]` takes custody of an orphaned child on its recording host. Claims use the live caller identity and preserve the original owner. Repeated claims by the custodian are harmless.
- `dogsled accept <item> --result <id> --evidence <text>` records a review of the exact returned result after a custody claim. The original task stays open. MCP exposes both commands. Pi records successful receipts as `dogsled.custody.v1` audit entries without changes to task state.
- The lead skill's small-change case now counts edits, not lines. The lead edits directly when it can name every edit before it starts and there are at most 3; an added test counts as an edit. In past sessions, the number of edits predicted a change's tool calls far better than its line or file count.
- Check-ins do not repeat orphan notices. Status lists each item once as `orphaned (unclaimed)`, `orphaned (claimed)`, or `unknown`.

## 0.26.1 - 2026-10-28

- `dogsled drop` accepts an orphan from any session when the origin proves its owner pane is gone or its session ended, by the sweep's custody rule. Unreadable evidence keeps the owner rule.
- `dogsled drop` fetches origin before it counts the discarded commits. A stale default branch counted commits that had already landed.

## 0.26.0 - 2026-10-28

- The web portal is redesigned around agents. The left navigation lists every repository with its agents, and workers sit muted under their lead. A top-bar Inbox counts reviews, questions, owner prompts, and unseen workers. A waiting plan review, question set, or key prompt fills that agent's screen. The conversation tab shows the agent's full history as markdown. Leads show their task title. Closed rows no longer list on the board. Question forms show their images.
- The Pi `interview` tool and `submit_plan` open the portal on the new question or plan.
- `dogsled drop <item-id>`, and the MCP tool `drop`, delete a closed child's retained local and origin branch on its owner's request. The item leaves the board. A closed item whose done mark is gone gets its branch checked at most once an hour, and it leaves the board when the branch is gone.
- A job item stays open until a sample taken after its launch lacks its pane. Before, a sample taken before the pane existed closed live jobs at start, so their owners never heard the job end.
- A job refuses to run an expired request or one it cannot prove, and it runs only with a matching durable claim.
- One owner derives the agent lifecycle state. A handoff counts as done only after the native `done` proves it.
- Owner custody has a durable record. Check-ins go to the child's custodian, which is still its origin owner until claims ship.
- Check-ins no longer repeat "Orphaned children the sweep keeps". `dogsled status` lists each orphan once, as orphaned or unknown.
- A plain remote error names its typed cause. Remote preparation reports its cause in the ticket and the owner inbox, and it fetches a base that the peer's clone has not fetched.

## 0.25.3 - 2026-10-27

- `dogsled web` finds Herdr's socket through `herdr status server` when `HERDR_SOCKET_PATH` is unset, so the launchd service can list and message agents.

## 0.25.2 - 2026-10-27

- The Linux release build compiles dx from source, because the prebuilt dx needs glibc 2.39 and the release builds on Ubuntu 22.04. The 0.25.1 tag built no Linux archive and published nothing.

## 0.25.1 - 2026-10-27

- Release builds bundle the web portal from the `web` package. The 0.25.0 tag built no archives.

## 0.25.0 - 2026-10-27

- `dogsled web` serves a local portal at `127.0.0.1:7465`. The `[web] port` configuration sets another port. The portal shows the board grouped by repository, todos, Herdr agents and their conversations, and a message box. A panel answers a blocked agent with the keys its prompt allows. The Rust/WASM Dioxus page is embedded in the binary. The server accepts only loopback Host values and the portal's own Origin. Pi tools authenticate with a per-user token in `<state>/web/token`.
- The Pi extension adds an `interview` tool with pi-interview 0.13.0's parameters. The portal shows the questions and collects the answers. Plan mode starts with `/plan` or `Ctrl+Alt+P`, limits writes to markdown, and adds `submit_plan`.
- The portal shows plans with a line map and annotations: text, images, and Quote in message. It compares revisions and accepts approval or requested changes. An approval emits `dogsled:plan-approved`, which pi-todo consumes.
- When `dogsled web` is not running, the Pi tools fall back to `dogsled interview` and `dogsled review`. These commands serve the same form or review once on a random loopback port.
- Release archives include `dev.dogsled.web.plist`. `dogsled web` exits promptly on SIGTERM and SIGINT, even with open streams.

## 0.24.0 - 2026-10-26

- The lead skill lets the lead make an edit itself in three cases. The first case is a specified change under about 100 lines. The second case is when no host can start a worker. The third case is a direct user order. Before, the skill said the lead never edits code, so a lead started one writer for each 10-line edit. When every spawn failed, the lead refused the user's direct orders to make the change. Now step 0 keeps that work in the lead's session. Step 1 puts the small edits of one request into one unit.
- The lead skill sends exec agents that run under prewalk away from the lead procedure. Those agents run their assigned work in the same session and do not delegate.
- Agent, job, and queued rows carry the canonical repository of their checkout, as `host/owner/name` from the origin URL. The ssh, scp-like, and https spellings of one remote give the same identity, and the Pi unit protocol reads the same fixture. A checkout whose identity is known costs no `git` call.
- Repository tooling moved to proto and moon. This change does not affect installed hosts.

## 0.23.2 - 2026-10-22

- The cleanup sweep no longer closes a pane whose root process is a running command, such as the `dogsled sidebar` pane. Before, a pane counted as an idle shell whenever no foreground process had a different pid from the pane's shell pid. Herdr runs a plugin pane's command as the pane's own root process, so the sidebar looked like an idle shell, and the sweep closed the sidebar and every workspace that held it. Now only a shell that waits at its prompt is idle: a root process with another name, a shell that runs a command such as `sh -c "..."`, and process information that does not name the root process all keep the pane.
- The idle-shell rule accepts only a shell that waits at its prompt, and a cleanup report names the program of a running command without its arguments. Before, any argument the rule did not recognise still counted as an idle shell, so a shell that ran a script such as `zsh build.sh` looked idle. Now a shell with an unknown option, a script, a command string, or malformed arguments keeps its pane. A report names the program only, so an argument with a secret no longer reaches it.

## 0.23.1 - 2026-10-22

- Fixed queued wait reasons to retain each host's reservation refusal. Transient SSH failures during reservation, checkout, or preparation retry with backoff. Remote command failures, authentication failures, invalid requests, conflicts, and deadlines remain terminal. Before preparation retries, the selected host revalidates the same reservation ID. An expired reservation that the host refuses keeps its request queued with the refusal reason. Preparation records its intent before side effects and refuses interrupted work without completion proof. Completed preparations still replay by request ID.

- A committed unit whose worker pane closed without a final reply now settles as `stopped`, so its owner can release the unit.
- The Pi inbox reader catches messages that arrive before its directory watcher starts. Before, a message that arrived between the first claim and the watcher's start got no change event and waited for the next write. Now the reader drains the inbox once more after it arms the watcher, so that message arrives without another write.
- Handoff handles a missing local branch ref without repeated errors. Before, each tick logged `cannot resolve integration tips` for every such branch (3352 lines in 11 minutes on netcup). Now handoff resolves the branch through `refs/remotes/origin/<b>` when its local ref is gone. A branch with neither ref settles and no longer appears as work to land. A closed, settled mark no longer refreshes on every tick.

## 0.23.0 - 2026-10-22

- Opening the sidebar no longer takes the focus from the pane that opened it. Before, `dogsled board --open` and `--toggle` opened the sidebar pane without focus and then swapped it into the leftmost slot, but a Herdr swap always focuses the pane it moves, so the sidebar took the keyboard and the pane that asked for the board lost it. Now Dogsled remembers which pane had the focus before the swap and gives it the focus back over the Herdr socket. A dock that is already open, and a toggle that closes one, change no focus.
- The Hosts panel shows memory and disk as two matching meters, each with a Nerd Font icon and no text label. Before, line 2 read `mem ▰▰▰▰▰▰▱▱ 76%  disk 22% free`: two words named the values, and the disk had no meter at all, only free-space text. Now the memory icon (`nf-md-memory`, U+F035B) and the disk icon (`nf-md-harddisk`, U+F02CA) name them. The disk meter fills by the share of the disk in use and its percent is that same share, so a full bar means a full disk as it already does for memory, and both meters mean the same thing. Each meter takes the colour of its own admission threshold, so a disk low on space turns yellow and then red like memory does. A narrow pane gives up the disk percent first, and then the disk meter.
- `unit settlements` and `unit observe` read only the caller's unacknowledged units, and a refresh reads each remote host with one `unit-receipts` ssh call instead of one call per unit. Before, every Pi prompt ran about 30 sequential ssh calls to netcup for every owner's units, which flooded its sshd (MaxStartups drops, `ssh netcup: other`) and made Pi wait about 22 s at startup and on each prompt. A 0.23.0 caller needs `unit-receipts` on its peers, so upgrade netcup and momokaya-2 before the Mac. A 0.22.0 peer is logged and skipped, not read.

## 0.22.0 - 2026-10-22

- `dogsled sidebar` replaces the text side board (`board --side --watch`, removed). It is a ratatui app with three framed panels: Hosts, Agents & Jobs, and Todos. The Hosts panel shows each host’s load, `▲`/`?` marker, memory meter, and free disk, coloured by the admission thresholds, from the cached status; the same typed `HostBrief` now feeds `status --line`. Colours come from Herdr’s `[theme]`. The Herdr pane is `sidebar`.
- pi-todo protocol v3: Pi publishes `display.sections` (`in_progress`, `pending`, `completed`, `abandoned`) instead of provider-rendered lines, and no longer reads a board width. A v2 file is refused. This needs `@oisin-ee/pi-todo` 0.10.0 or newer (`@oisin-ee/oisinpi` 0.10.0). The service caches the sections in `todo-sections.json`, so a sidebar that starts before the service upgrades does not read the old `todos.json`. When the todo cache cannot be read, the sidebar still draws Hosts and Agents & Jobs and shows the error in its footer.
- A committed unit whose ticket ended `failed` or `cancelled` before any launch request was saved now settles as `not_started`, with the ticket and its error as the reason. Before, `unit observe` showed `unresolved` with no settlement for ever, so the owner's todo kept its claim and could not retry (for example after `ssh netcup: exit 255`). Refresh, `unit observe` and `unit settlements` write the event under the admission lock, and the owner receives it through the normal settlement delivery. A ticket that is still pending, or one that failed after a launch request was saved, is not settled: the worker may have started, so only host evidence can settle it. A settled unit cannot be committed again; the owner prepares a new revision.
- `dogsled legacy-continue <pane>` continues a seeded worker whose launch record has no owner host, on the origin host only. Such a record comes from a release before the owner host was stored, and `send` refuses every sender for it, so a worker that called `done` before its unit returned had no way to continue. The command accepts input only from the owner pane that runs the admission's origin Pi session now, as its transcript header shows. The original admission must be on this host and must have selected it. The admitted request, the saved launch, the host launch record, and the seeds must be equal, and the launch must be started and unreturned. The host checks all of this again under the admission, done, and launch locks, in that order, on every attempt. Replay protection belongs to the unit execution: before any input exists, the host writes a receipt in `launches/<request>.continuations.json` that names the exact done mark, and nothing removes it. Then it stores the input, records the outbox entry, accepts the receipt, and removes that mark. A retry after a stop at any step finishes once and returns the first record. A replay after a later `done` is refused and keeps the later mark, also after the inbox removed the first record after 24 hours. A prepared receipt whose mark is gone refuses, and puts no input. Nothing writes an owner host, a relaunch, a seal, a return, or a settlement. A remote worker or remote admission is refused, and `send` is unchanged.
- A seeded worker that calls `done` before its unit returns is no longer stuck. Before, the first idle sealed the mark. Then the inbox refused every message with `finished; start a new child`, and the check-in showed `finished`. But the handoff never closed the pane, because no terminal receipt existed, and `unit observe` stayed `running`. Now the worker host seals and finishes a seeded mark only after the launch has a terminal receipt. Until then, only the execution owner can continue the worker. The owner's message or task removes the mark, and the same pane, session, worktree, and branch continue. The host refuses input from other senders, and the worker's own claim, without a change to the mark or the inbox. Missing, unreadable, or mismatched launch evidence refuses input and never closes the pane. A mark that an earlier release sealed without a receipt follows the same rule. Nothing writes a seal, acknowledgement, acceptance, or settlement. The owner is its pane ID and hostname, both stored in the launch record, and the host compares the full sender before any change. A pane ID alone is not enough: the same pane ID on another host is a different agent. A launch record without an owner host refuses every sender.
- The `prune` path now treats a commit made after the plan as a skip too, as the removal recheck already did. Before, `apply_one` compared the whole planned tree, so a moved `head` on the same repository, path and branch was reported as `planned worktree identity changed` and alerted the owner. Now the entry is skipped with `refuse <id>: HEAD moved since the survey (<old> -> <new>)` and the worktree stays. A branch entry whose tip moved after the plan is skipped the same way, instead of `branch identity changed`. One `Tree` comparison serves all three checks. A changed repository, path or branch, or a tree that appears or disappears, is still a failure.
- An SSH read error now holds only a safe category, such as `ssh peer: connection refused`. Before, `Error::Ssh` carried raw remote stderr and remote error text in its `Display`, so a secret in a banner or a failing command could reach ticket errors, inbox handoffs, unit-evidence logs, and unit JSON. A remote deadline error no longer repeats the remote text either. The raw text is written only to the local file named by `DOGSLED_SSH_DEBUG_FILE` (mode 0600), and only when that variable is set. A peer's own refusals now carry a structured `code` in their JSON error (`NO_INBOX_READER`, `CLONE_FAILED`). The caller maps a known code to a fixed message, so a missing inbox reader and a failed clone still say what happened; unknown remote text shows only as `remote command failed`. The clone failure shows no git output. Older ticket and unit records are not rewritten.
- A launch into a pane whose shell is still starting no longer closes that pane or its workspace. Herdr answers `agent_pane_busy` while shell init children hold the foreground, and its own retry lasts 2 seconds; under load that is too short. Before, Dogsled took the answer as a final refusal and closed the pane, which removed a workspace that held only that pane. Now Dogsled repeats `agent start` on the same pane with a pause of 250 ms growing to 2 s, for about 20 seconds. If the pane is still busy, the launch fails with `<pane> was still not an available shell after <n>s; the pane stays open`, records `attempted=false`, writes no receipt, and the next attempt uses the same pane. Herdr still decides readiness; Dogsled does not poll process information. Other refusals are final as before.
- A cleanup close of a workspace that Herdr already closed (`workspace_not_found`) counts as closed, not as a failure. Herdr closes a workspace with its last pane. Other close errors still fail.

## 0.21.0 - 2026-10-22

- A long task launches reliably. Herdr types the launch as one terminal line of at most 1024 bytes, and the child contract alone used about 600 of them, so a task of a few hundred bytes failed with `agent_start_input_failed`. The task now always goes to `launches/<id>.task.md`, and Pi and Claude read the contract from `launches/<id>.contract.md`, so the line no longer depends on the task. The host that runs the child checks the line against its own paths before it creates a pane; the caller's paths never refuse a peer launch.
- Prune and the sweep no longer plan to remove a worktree while the host snapshot reports a shell in it idle for less than 15 minutes. Before, the worktree that `place` made held only an idle shell, so a pass could plan its removal seconds after the owner started work there, and the apply-time check then failed with `worktree identity changed` when the owner's first commit moved HEAD. Such a shell now holds the checkout until it has been idle for 15 minutes, the shell rule's age; a shell with no reported age holds nothing, as before. An explicit `close` and the identity check are unchanged.
- A cleanup recheck that finds a new commit in a planned worktree after the survey is now a skip, not a failure. Before, the pass reported `worktree identity changed` and alerted the owner, although only `head` had moved. Now the entry is skipped with `refuse <path>: HEAD moved since the survey (<old> -> <new>)`, the worktree stays, and the next survey re-plans it. A changed repository, path or branch, a different main checkout, or an empty head is still a failure. The recheck is as strict as before: a moved worktree is never removed in that pass.
- Cleanup messages reach only the agent that created the work: the owner of an open item that holds the worktree or pane, or the agent whose recorded launch started a child in the worktree. Before, every pane Dogsled did not launch was an operator and heard host-level failures, repository warnings, and unowned holds. Every pane in the same repository heard landed-dirty findings. So research agents that created nothing got cleanup messages and continued turns. The prompt-context cleanup line now counts only the caller's findings. Host-level failures and repository warnings have no creator: they stay in `dogsled status` and `status --brief`.
- `dogsled board --open` now docks the board in the leftmost slot of the tab, 38 cells wide, beside Herdr’s own sidebar. Before, it split the owner pane 50/50 on the right. It splits the tab’s top-left pane, swaps the new pane into that slot, and sets the split ratio over the Herdr socket. It never re-creates other panes. A new `toggle` plugin action (`dogsled board --toggle`) closes this tab’s live board or opens it. A Herdr failure is an error, with no fallback to the old split. `--open` and `--toggle` now need `HERDR_SOCKET_PATH`.

## 0.20.0 - 2026-10-21

- The Herdr plugin installs the checksum-verified release binary for its own version at `bin/dogsled` during install, building from source only when no archive exists, and its board pane runs that binary directly. Before, a shell wrapper ran whichever `dogsled` was on the Herdr server's `PATH`; an older binary rejected `--side` and the pane closed at once with no message. The shell wrappers are gone.
- Dropping a worktree registration matches the registered path and Git's path after both resolve symlinks. On macOS, a registration under `/var` now matches Git's `/private/var` path, so cleanup no longer keeps a stale registration after it reconciles a vanished worktree.
- A cleanup pass no longer fails with `cannot check worktree <path>: fatal: cannot change to <path>` when another actor removes a worktree after the pass lists it, but before the dirty check that runs `git status` inside it. This applies to the survey and to the apply-time recheck. After a Git error there, cleanup lists the worktrees again. When Git no longer lists the worktree and its path is absent, the survey plans a registration drop, and the recheck drops the registration and reports the worktree as already removed. Cleanup does not match Git's error text. A worktree whose directory remains after Git lost it is still refused. Any other Git error in an existing checkout still fails.
- A cleanup recheck that refuses an entry because its state changed after the plan is a skip, not a failure. Live work can make a planned worktree dirty or held, or a pane busy, before the removal. The entry stays, and the pass and prune list it as `skipped:` with the same `refuse` reason. `Applied` has a new `skipped` list, and the stop gate and the status failure count ignore it. Identity changes, missing entries, command errors, and unreadable evidence still fail. The recheck is as strict as before.
- An explicit `dogsled close` of a pane in a shared checkout closes only that pane. Before, when the other agents in the checkout were idle, it closed their panes and removed the checkout without warning; only automatic close kept a shared checkout. The checkout now goes with the last pane that uses it.
- A launch without a final receipt no longer holds its preparation once its recorded terminal is gone from its pane, or another terminal replaced it. Start and recovery refuse such a pane, so the launch can never use its checkout. A launch whose request lock is held, whose matching agent still runs, or that has no saved terminal identity still holds. No launch record is rewritten.
- A cleanup pass no longer fails with `worktree identity disappeared` when another actor removes a planned worktree after the survey. When Git no longer lists the worktree and its path is absent, the pass drops the registration and reports it as already removed. A path that Git no longer lists but that still exists is still refused.
- Cleanup no longer runs `git worktree remove` on a worktree whose directory remains after Git lost it: Git does not list it (for example after `git worktree prune`), calls it prunable, or its `.git` file is gone. Before, a pass could unlock it and fail with a raw Git error such as `validation failed` or `is not a working tree`; after a prune, the survey ignored it without a word and kept its registration. Now prune and the sweep report it as an `unlinked` hold, for example `worktree unregistered by git; directory remains: <path>`, with the owner's action. Status, stderr, and the stop gate show it; the gate speaks once per set. `close` refuses it before it closes or unlocks anything. Dogsled never deletes the directory. After the owner moves or deletes it, the next pass drops the registration.
- Loss checks refuse a path without its own worktree identity (its own `.git` and a matching Git top-level path) before they read status. Before, a worktree whose `.git` file was gone inherited the main checkout's untracked files as its own dirt and showed as landed-dirty.
- Cleanup failures and `unlinked` holds go to the affected item’s live owner. Unowned ones and host-level repository warnings go only to operator panes, never unrelated children.
- MCP `spawn` returns a durable ticket before agent startup. The admission service confirms startup outside the MCP call budget. CLI agent launches keep their synchronous result.
- Startup errors include recent pane output in caller or ticket errors and launch progress records. Native refusals, unknown outcomes, and recovery safeguards remain unchanged.

## 0.19.0 - 2026-10-21

- `dogsled quota` reports every enabled CLIProxy credential that CLIProxy marks `supports_quota`, other than Claude and Codex, through CLIProxy's `quota/fetch` route. Each bucket is a row with the lowercased provider as its pool. A failed fetch gives an error row for that account. Snapshot version 1 and the Claude and Codex rows are unchanged.
- Quota no longer discovers MiniMax through `claude-api-key` or parses `token_plan/remains`. A MiniMax CLIProxy plugin appears on the board without a dogsled change.
- Settled preparations no longer hold landed checkouts. A preparation that fails releases its own hold. Cleanup and close ignore a hold whose launch has a final receipt, or whose local preparation failed. A legacy hold without a request id ends when any launch of its checkout is final. Interrupted preparations, launches without a final receipt, and seeded unit refusals that their peer has not settled still hold. No state file is rewritten.

## 0.18.1 - 2026-10-21

- Prune and automatic cleanup remove remembered repository and worktree registrations only when their paths are absent. Removal rechecks absence under the registry locks and deletes no checkout data. Doctor lists the removable records without changing them. Configured paths, discovered paths, unreadable paths, and dangling symlinks remain report-only.

## 0.18.0 - 2026-10-21

- `dogsled status` now says `unreachable: <category>` for a peer that SSH cannot read, with its last snapshot age. The category is one of timeout, connection refused, host not resolved, authentication failed, remote command failed, unexpected output, deadline reached, or other. Status never shows raw SSH or remote error text, which can hold secrets. `service: down` stays only for a host that answers while its service is not running. A sleeping peer no longer looks like a stopped service.
- Agent items now retain the child host's launch-confirmation timestamp. Reconciliation ignores absent-pane snapshots from before confirmation, including late remote admissions and clock skew. Legacy items and job completion keep their existing rules.
- `done` is a durable terminal handoff. It checks worktree safety before acceptance, survives restored terminals, and rejects messages after the final turn. The child host closes its pane and agent worktree while retaining its branch. Closed branches stay on the owner’s TO LAND board until integrated or deleted. The close path shares cleanup’s linked-workspace group hold.
- Herdr now hosts a right-side Dogsled board with cached quota age, stale marking, work rows, and pi-todo’s unchanged rendered lines. Pi reuses each tab’s split and keeps the existing widget only without reachable Herdr. A pane Dogsled launched never opens a board: it keeps the widget, and its todo lines appear on the parent's board. `/dogsled` remains available in both modes.
- Launch records with obsolete terminal identities no longer hold restored panes. Empty shells use the existing 15-minute cleanup rule. Started receipts, active panes, and locked recovery checks remain protected; launch evidence stays unchanged.
- Cleanup keeps idle primary workspaces while linked workspaces remain open and explains the group hold. It also keeps their idle panes because closing the last pane can close the group. Each survey reads Herdr workspace provenance once; closure rechecks preserve the same safeguard.

## 0.17.0 - 2026-10-21

- A profile's `prompt` can be a list of files. Each one is appended to the system prompt in order, so team profiles share a common prompt and add their own.
- `dogsled quota` collects Claude, Codex, and MiniMax quota on a configured Mac through a Keychain-only management key. Versioned snapshots refresh every five minutes, persist atomically, and reach enabled peers over SSH. Cached reads never contact the provider. Account errors exclude credentials and response bodies.
- Pi runs the stop gate on every completed stop. Before, it skipped stops that ended on the assistant's message, which is every normal stop in Pi 1.3, so Pi agents never requested a cleanup pass or heard gate reminders.
- Worktree removal uses the existing 10-minute setup limit instead of the 30-second routine limit, including cleanup after failed setup.
- Pure tracked deletions lose nothing when the default branch or refreshed remote refs hold HEAD. Cleanup restores only those deleted paths before unforced removal. Other dirt still prevents removal. Board facts use the same loss rule.
- Squash detection compares the branch's net paths and before/after objects. Merges from main and reverted edits no longer block exact squash proofs.
- A tip also counts as landed through an integrated local or remote-tracking ref that contains it. The result names that ref.
- Prune and sweep use one idle-pane rule. Unfocused shells without Dogsled records close after 15 minutes at an empty prompt. Caller panes and agents without `done` stay open.
- Final launches release their verified empty panes and checkout holds. Prepared-pane refusals close their own idle terminal. Recovery retains launch receipts and checks ownership under both launch locks.
- Service status uses the service lock instead of snapshot age. Status shows age separately, and finished children can close despite late snapshots. Admission and freshness-sensitive readers retain the 30-second limit. Older peers retain age-based liveness during upgrades.
- Tick observations deduplicate shared checkouts and bound parallel checkout and shell-pane reads by available CPUs. Dirty checks and automatic-close guards remain unchanged.

## 0.16.4 - 2026-10-20

- Explicit close can retire the verified idle shell of an interrupted launch without deleting its checkout or inventing a task outcome. Unknown launch evidence and preparation holds remain intact for normal cleanup.
- Agent starts and interrupted-launch recovery share a pane lock. Recovery rejects competing ownership, changed terminals, agents, and incomplete foreground evidence. Automatic cleanup retains its existing safeguards.

## 0.16.3 - 2026-10-20

- Proven native launch refusals now settle managed executions as `not_started`, without accepting their tasks. Evidence must match the saved launch, seed, execution, and commitment; unknown outcomes remain unresolved.
- Preparations track their request owner. Refusal recovery releases only the matching hold; legacy holds without ownership evidence remain protected.
- Invalid evidence is reported per request without blocking unrelated settlements or maintenance. Local storage failures remain fatal.

## 0.16.2 - 2026-10-20

- Managed workers use deterministic 32-character agent names derived from their full task and execution references. UUID-shaped task identifiers now fit Herdr’s native name limit. Full binding checks remain unchanged; name collisions still produce a final refusal.
- Previously committed launch names are not migrated or replayed automatically.

## 0.16.1 - 2026-10-20

- Agent launches use Herdr’s native shell-readiness handling. Dogsled no longer rejects a newly opened shell while its initialization commands run. Terminal identity, foreign-agent checks, and unknown-outcome protection remain enforced.
- After a final launch refusal, Dogsled closes only an allocated pane that still has the expected terminal and a positively verified idle shell. Occupied, replaced, uninspectable, and caller-provided panes stay open.

## 0.16.0 - 2026-10-20

Repository work opens with its repository. Pi gains the structured task handoff bridge.

- Jobs and readers use their worktree’s workspace, then the main checkout’s workspace, or a new repository workspace. Remote launches follow the same rule. Branch agents still use their worktree workspace.
- The Pi bridge prepares and claims a task from pi-todo in one spawn call. It validates worker imports before a model request and delivers sealed results separately from inbox messages. Lead acceptance remains explicit.
- Runtime recovery reconciles uncommitted preparations after reload and lost prepare responses. Unknown outcomes retain their claims; recovery does not redispatch work.
- Dogsled alone registers `--todo-unit-file` and exposes the versioned seed-source capability. A compatible pi-todo provider consumes it and owns import validation.
- The Pi extension now requires Pi SDK/TUI 1.3 or later within major version 1, and Node 22.19 or later.
- Cross-repository tests load both real extensions in both orders. Executor transport is simulated in those tests; live deployment verification is separate.
- The cleanup-wakeup test uses a valid short socket path. Deadline-test failures now include the command output.

## 0.15.0 - 2026-10-20

A child finishes when it says so, not when it goes idle. A refused launch no longer holds its worktree.

- Herdr's `idle` and `done` only mean "ready for input". The sweep closed children that asked a question, orchestrators that waited for their own children, and fresh children after their first turn.
- A child calls the new `done` MCP tool, or `dogsled done`, in the turn that completes its task. The sweep closes it only after that turn ends, and only when the loss rule allows. Orphans follow the same rule.
- `done` is refused while the caller owns open items or queued tickets. A later turn, or an inbox record delivered to the child, ends the mark.
- Dogsled tells each child this contract at launch: Pi and Claude through `--append-system-prompt`, Codex through `-c developer_instructions`.
- A turn that ends without `done` still sends its reply to the owner's inbox, and says that the child waits for input.
- Breaking: an idle agent that has not called `done` holds its checkout in prune, the sweep, and an automatic close. The board shows it as running, not TO LAND. Only an explicit `dogsled close` takes it.
- Queue order holds per host. A full host no longer holds back a ticket pinned to another host with room. Before, the first ticket that found no room ended the admission pass for every host.
- A host with a stale snapshot gives the snapshot's age in its reason, such as `service: down (last snapshot 44s ago)`. The `service` field is still `down`.
- Herdr can refuse `agent start` before it types into the pane, for example with `agent_pane_busy` or `agent_name_taken`. Dogsled now treats that refusal as final. It records the refusal as the launch result, closes the empty pane it created, and does not retry. Before, the retry reported "startup outcome is unknown", and the unfinished receipt held the worktree forever.
- An unfinished launch holds its checkout only while its pane exists. A receipt whose pane is gone no longer blocks `close`, prune, or the sweep.
- Groundwork, not yet usable: the `dogsled unit` commands and the unit forms of the `spawn` tool implement the unit request protocol in `docs/unit-requests.md`. They need the Dogsled Pi bridge, which is not released. Without it, a unit spawn returns `PROVIDER_UNAVAILABLE`.

## 0.14.0 - 2026-10-20

Cleanup uses one loss rule, and the Stop gate starts each cleanup pass.

- One rule decides if removal loses work. It applies to worktrees, detached HEADs, branches, `close`, orphan close, and board facts. The checkout must be clean, and the default branch or freshly fetched remote refs must hold every commit. Ignored files do not count.
- Each pass runs `git fetch --all --prune` first. If the fetch fails, the repository gets a warning, and only the default branch counts.
- Branches with a deleted upstream get no special rule. They go when the loss rule permits removal.
- Prune and the sweep share one list of holds: active panes or processes, the caller, an unreadable process table, open agent items, pending preparations and launches, and foreign locks. An idle shell, an idle agent's own processes, or a workspace alone no longer holds a checkout. The "untouched for a day" rule is removed.
- A missing or unreadable repository is a warning. The survey continues. Before, one missing repository stopped doctor, prune, and every sweep.
- An unheld dirty checkout whose work is in the default branch is a `landed_dirty` finding, with up to ten dirty paths.
- The 10-second sweep in `serve` is removed. The Stop hook writes `sweep-request.json` and wakes the service through `sweep.sock`. The service runs one pass for each request. A socket failure is a warning, and the request file stays the source of truth.
- Each pass is recorded in `sweep.json` and in `sweeps.jsonl`, which keeps seven days. `status` shows the latest pass. The gate reports failures, repository warnings, and landed-dirty findings once for each distinct set. Successful removals give no reminder.
- Breaking: `close` refuses local-only work, also for an explicit owner close.

## 0.13.1 - 2026-10-19

- Agent admission retries retain the selected host, prepared workspace, and original terminal identity. They do not prepare the branch again.
- Interrupted starts reconcile a matching agent or a saved success receipt. Unknown outcomes fail without sending the task again.
- Replaced terminals and foreign agents are refused. Interrupted jobs remain inspection-required instead of being restarted.
- Existing partial placements are preserved and are not adopted automatically.

## 0.13.0 - 2026-10-19

wrangle is now dogsled. The old name was too close to Cloudflare's wrangler. Nothing reads the old names, so update every host at once.

- The binary, crate, and repository are `dogsled` (`oisin-ee/dogsled`). The Pi package is `@oisin-ee/pi-dogsled`.
- The Claude and Codex plugin is `dogsled@dogsled`, the Herdr plugin is `dogsled`, and the MCP server is `dogsled` (`mcp__dogsled__*`).
- The environment variables are `DOGSLED_CONFIG`, `DOGSLED_STATE`, and `DOGSLED_BIN`. A repository's worktree file is `dogsled.toml`.
- Configuration and state live in `~/.config/dogsled` and `~/.local/state/dogsled`. Move the old directories while the service is stopped. The service units are `dev.dogsled.plist` and `dogsled.service`.

## 0.12.3 - 2026-10-19

- The Pi work widget uses the wrangle board design again: a header with counts and a load meter for each host, then the current task, then agents, jobs, and queued work grouped TO LAND, ATTENTION, RUNNING, and WAITING with aligned harness·model, host, activity, and age columns. This replaces the compact WORK strip from 0.12.1.
- Alt+W and `/work` still expand the widget in place. The expanded widget keeps the board's header, groups, and columns, and adds a view line, a selection marker, and key hints.
- The Todos view hosts the provider-owned pi-todo view inline.

## 0.12.2 - 2026-10-18

- The compact Pi widget shows the current task and named agents and jobs above the editor.
- Alt+W and `/work` expand or collapse the same widget. Work details stay inline. The editor draft stays unchanged.
- Modified shortcuts navigate work and run explicit actions without taking ordinary typing from the editor.
- Pre-commit checks select Rust and Pi verification from staged paths. Changes to shared check configuration still run both gates.

## 0.12.1 - 2026-10-18

The 0.12.0 release was cancelled before publication to synchronize the Cargo lockfile.
The shared work UI ships in 0.12.1.

- Pi shows one WORK strip for branch-backed todos and owner-pane agents and jobs. Compatible pi-todo providers retain standalone display when the shared presenter is absent.
- `/work` opens a bounded keyboard-operated overlay. Inspect read-only todos, agent output, and retained job tails; confirm job stops and queued cancellations, or send inbox messages. `/wrangle` retains administration.
- Job guidance uses non-blocking Wrangle commands instead of background-task tools. Execution and completion delivery retain their existing owners.

## 0.11.0 - 2026-10-18

Nothing types into an agent's terminal any more.

- `wrangle hook inbox --harness claude|codex --event Stop|UserPromptSubmit` reads the inbox for Claude Code and Codex. The plugin registers it on both events. At `Stop` records continue the turn (`decision: block`); at `UserPromptSubmit` they are `additionalContext` beside the user's text. An idle Claude or Codex agent sees a record at its next prompt or stop.
- Watchdog check-ins are `check-in` inbox records, not `herdr agent prompt`. An owner without an inbox reader gets none until it has one. A finished child's reply comes only from the sweep; its check-in is no longer immediate. `watchdog.json` drops `deliveries`.
- Admission notices go only through the inbox, to working owners too.
- The Pi extension is published to npm as `@oisin-ee/pi-wrangle` with each release, after the binaries; `pi install npm:@oisin-ee/pi-wrangle@<version>`. The package holds only `pi/` and the skill. At session start it warns when `wrangle --version` differs from its own version.

## 0.10.0 - 2026-10-18

Wrangle replaces Shepherdr for delegation: one board, one inbox, and children that close on events.

- `wrangle agent --kind pi|claude|codex` passes the task as a launch argument, or as `launches/<id>.task.md` when it has a newline, a leading `-` or `@`, or more than 4000 characters. `--message-file` reads it from a file or stdin. Nothing is typed into the new pane. A failed `herdr agent start` is retried with backoff, three tries in all, then the owner's inbox reports the failure.
- `--profile NAME` (Pi) reads `~/.config/wrangle/profiles/NAME/profile.json` on the child's host and passes `--model`, `--thinking`, `--append-system-prompt`, and `pi_args`. Shepherdr profiles load unchanged. `wrangle profiles` lists them.
- The inbox: `wrangle send TARGET [--host H] --message|--message-file`, `inbox claim|ack|list|path`. Records live per Herdr terminal on the target's host; a claim is a lease, offered again with backoff until acknowledged. A target without a reader fails at once and the sender hears so. The service gains an inbox lane.
- Event-driven close: when a child's turn completes and it is idle or done, the sweep captures its final reply, puts it in the owner's inbox, and closes a clean reader or a writer whose branch has landed. A writer with commits waits under TO LAND; blocked and working children never close; nothing closes on a timer. A blocked child sends one `blocked` record per episode.
- `wrangle board [--owner|--all] [--json]`: an owner's children, jobs, and tickets grouped TO LAND, ATTENTION, RUNNING, and WAITING, with harness, model, and activity. The prompt context's close commands become one `To land:` line.
- `wrangle read TARGET`: a child's or job's newest output and its captured reply.
- `wrangle mcp` serves `status`, `spawn`, `run`, `send`, `inbox`, `read`, `close`, `stop`, `cancel`, and `place` as MCP tools over stdio. The Pi extension, the Claude plugin (`.mcp.json`), and the Codex plugin (`codex-mcp.json`) register it. No tool waits.
- One backoff (10 s, doubling with jitter, 5 min cap) for inbox leases, follow-up, peer reads, and launch retries. Host reports gain `retry_ms`; `wrangle retry HOST` reads a host again now.
- `wrangle machine list|rename|enable|disable|reconnect|add` manages Herdr's saved machines; `reconnect` and `add` open a tab for SSH prompts.
- Pi: a board widget replaces the items widget; `/wrangle` opens a control panel (Status, Hosts, Profiles, Inbox); the extension reads its inbox, injects records as `wrangle` messages (steer while running, a new turn when idle), and acknowledges them once Pi has saved them. `@earendil-works/pi-tui` is a peer dependency.
- Breaking: the Pi extension no longer tracks or places Shepherdr `agents spawn` calls, and `pi/placement.ts` is gone. Use `wrangle agent` or the `spawn` tool.
- `close` removes only agent worktrees (locked `agent:worktree`). A child in any other checkout, such as its owner's, loses only its pane. Before, closing such a child removed the owner's worktree.
- Probe agents gain `terminal_id`, `completion_seq`, `cwd`, the session's model and activity, and worktree facts (branch, ahead, dirty, landed). Items gain `harness`, `profile`, `model`, `role`, and `reply`; old `items.json` still loads.

## 0.9.1 - 2026-10-16

- Admission counts every active agent (live and not `idle`) as declared demand for as long as it runs, not only for its 90 s reservation. A host now needs `cores − reservation_load × (active agents + reservations)` of at least `free_core_fraction_min × cores`, and total memory of at least `memory_reserve_bytes × (active agents + reservations + 1)`, as well as the measured CPU and free-memory rules. Headroom is the smaller of the measured and declared free cores. An agent waiting on its model used almost no CPU, so a host with many agents looked free between builds; netcup admitted 13 agents in 32 minutes and ran at busy p90 0.95. The refusal reason now names the active count.
- `place`, `run`, and `agent` queue a new request when a ticket is already pending, so the service admits tickets in order. Before, a new request could take a host slot ahead of waiting tickets.

## 0.9.0 - 2026-10-15

- `serve` sweeps on its own action worker, beside the watchdog and admission. Each pass builds the doctor plan in sweep scope and applies only entries whose removal loses nothing: ended job tabs and shells in deleted directories (and a workspace made only of them), clean integrated worktrees and clean detached worktrees whose HEAD is on a remote when no pane, workspace, or process holds them, and local branches with no worktree whose every commit is on a remote ref. It uses the same checks as `close` and `prune`, never touches the service's own pane, workspace, or directory, logs each removal as a `wrangle sweep:` line on stderr, and continues past a failed entry. Doctor and `prune` behave as before; everything else stays for `prune`.
- The sweep closes an orphaned child as `wrangle close` would when its owner pane no longer exists, its agent is idle or done, and its worktree is clean with every commit on a remote ref. A remote child closes with the new `host close --pushed`. A dirty, unpushed, working, or blocked orphan stays open. Current status lists each item once as `orphaned (unclaimed)`, `orphaned (claimed)`, or `unknown`. Check-ins no longer repeat orphan notices. Status JSON gains `orphans`.
- Doctor reports a `targets` class: build output, a directory with a `CACHEDIR.TAG` cache-directory signature directly under a main checkout or linked worktree, such as Rust's `target/`. It is removable when no workspace shows the checkout and no pane or process works in it; only `prune targets` removes it, after it checks the tag again.
- Doctor also reports a branch as integrated when a clean `git merge-tree` result equals the default branch's tree. The detail names the merge-tree rule. Conflicts and changed trees do not satisfy this rule. Integration checks disable external diff tools and text conversion. The merge-tree check uses an isolated object store without user configuration or attributes.
- Close each job tab after `exec` saves the final record and reports completion. `wait` and `logs` still work after the tab closes.
- Log pane-close failures to stderr without changing the job result. Prune retains its ended-job-tab fallback.
- Create and remove worktrees natively, without `wt` or host `mise` tasks. An optional `wrangle.toml` in the repository's main checkout sets `[worktree]` `dir` (default `.worktrees`), `setup`, and `teardown`. Commands run with `sh -c` in the worktree and can use `{main}`, `{path}`, and `{branch}`, each quoted as one shell word. Unknown placeholders and unknown keys are errors. A missing file means the defaults.
- `place`, `agent --branch`, and `run --branch` run `git worktree add` (a new branch from `--base` or the default branch, or an existing branch), lock the worktree with the reason `agent:worktree`, run `setup`, and open it with `herdr worktree open --no-focus`. A failed `setup` removes the new worktree and the branch it made, and placement fails. When the main checkout had no Herdr workspace, wrangle records the anchor in `.git/agent-worktree-anchor`. Refusals match the old `agent:worktree` task: an invalid branch name, `--base` for an existing branch, a foreign lock, and a workspace that already shows the worktree.
- `close` and `prune` run `teardown` in the worktree first, so a failure keeps the worktree, its panes, and its lock. Then they close its panes and workspace, unlock, and run `git worktree remove` without force. The branch is deleted only when the integration check finds it integrated, so a squash-merged branch now goes too. After the last child, the anchor workspace closes when it is unfocused, is not the caller's, and has one pane with no agent.
- `close` also refuses a worktree outside the repository's worktree directory and a caller whose directory is inside the worktree. The JSON `worktree` result gains `anchor_closed` when the anchor closed.
- Doctor lists detached-HEAD worktrees. One is removable when it is clean, no pane holds it, and a remote branch holds its HEAD. `close` refuses a detached worktree and points at `prune`.
- Breaking: remove `hooks.prepare` and `hooks.remove` from the host config. A `[hooks]` table is now an unknown-key error. Move repository remove hooks, such as `.config/wt.toml` `[pre-remove]`, to `wrangle.toml` `teardown`.

## 0.8.2 - 2026-10-15

- Prune closes an ended job's tab at once when its shell is at an empty prompt. The job log keeps the output. A shell in a directory that no longer exists also closes at once. Other idle shells still need a day.

## 0.8.1 - 2026-10-15

- Doctor's process scan tolerates panes that close after the pane list. All process queries share the same closed-pane handling.

## 0.8.0 - 2026-10-15

- Doctor no longer lists a new worktree as merged when its base moves on. A worktree whose HEAD has not moved since Git made it, on a branch with no commits beyond the base, holds no work: it is ordinary work for its first day, then removable. Before, its branch counted as merged as soon as it was an ancestor of the base, so a worktree an agent had just made was removable.
- Doctor and prune no longer stop when a pane closes between `herdr pane list` and `herdr pane process-info`. A closed pane runs nothing.
- Admit on free cores and free memory instead of the one-minute load average. Free cores are `cores × (1 − busy) − reservation_load × reservations`, busy being the CPU busy fraction over the last probe interval (`/proc/stat` deltas on Linux, `iostat` on macOS). A host needs `free_core_fraction_min × cores` free (default 0.25), `memory_reserve_bytes` free per reservation plus one (default 3 GiB), and macOS memory pressure below critical. On the measured fleet the load rule refused the Mac while it was free 84% of the time; the research note in `docs/research/2026-10-15-admission-metric.md` records the evidence. Config keys `load_per_core_max` is gone; `free_core_fraction_min` and `memory_reserve_bytes` replace it (unknown keys are still errors).
- Probe `busy`, `cpu_ticks`, and `memory.pressure_level`; status lines show busy cores over cores. With a finite cgroup limit the probe reports whichever of the cgroup and `/proc/meminfo` has less free memory.
- Append every probe to `~/.local/state/wrangle/samples.jsonl` (seven-day window) so thresholds can be re-scored on real data.

## 0.7.3 - 2026-10-15

- Find a stacked branch as integrated when each branch in the stack landed as its own squash. A branch whose commits split into consecutive runs, each applied by one commit on the default branch, is integrated; doctor names every applying commit.
- Doctor finds more cruft. New classes: `branches` (local branches without a worktree, removable when integrated or fully on a remote), `workspaces` (a workspace whose every pane is idle closes whole), and `repositories` (a configured path that is not a checkout).
- Doctor finds a shell pane idle for 24 hours. The service records when each shell last had no foreground command, because Herdr reports no activity time.
- Doctor removes a worktree with no commits a day after Git made it, and reports unused worktrees with unmerged work: commits ahead, commits on no remote, and the last commit's age.
- Doctor reads repositories from Herdr workspaces and the new `[cleanup] repositories` policy, so repositories without a live pane are checked.
- The integration search merges each path on its own and reads each commit's changes from one `git log`. On a host with 250 branches, doctor took 13 seconds instead of 7 minutes.

## 0.7.2 - 2026-10-14

- Let prune try every entry. A failed or refused entry no longer stops the rest; prune reports each failure and exits with status 2. `--json` returns `{results,failures}`.
- Drop a worktree's registration when `close` removes it, so doctor no longer lists it as a stale registration.
- Show the last lines of a failed remove hook's output, not only the task runner's summary line.

## 0.7.1 - 2026-10-14

- Never list a repository's main checkout as a cleanup candidate, even when it lags the remote default branch. List each linked worktree once when the repository is registered through a symlink.

## 0.7.0 - 2026-10-14

- Skill: spawn a placed agent with `placement=pane pane=<pane_id>`. Without `placement`, Shepherdr ignored the prepared pane and opened a tab in the caller's workspace.
- Pi: give every `agents spawn` without a `placement` its own worktree workspace. Use the prepared or open worktree pane, or place a new `agent/<label>` branch with `wrangle place`. A queued or failed placement blocks the spawn with its reason.
- Find integrated worktrees from Git alone: a deleted branch, an ancestor of the remote default branch, or a branch that one commit there applies (merge, squash, or cherry-pick). A branch on the base tip is not a candidate.
- Do not count a shell with no foreground command as holding a worktree. Prune closes such panes and the worktree's workspace before removal. Doctor reports held integrated worktrees with the reason.
- Add `hooks.remove` (`{branch}`, `{path}`), default `mise run -q agent:worktree-remove -- {branch}`. `close` and `prune` both remove through it; wrangle no longer runs `wt` or `git worktree remove` itself.
- Let `close` finish a child tab in the caller's own workspace. Refuse only the caller's own pane, a worktree the caller works in or shows, and busy panes in the worktree.
- Print the full `wrangle close PANE --host LABEL` command for each finished child in `status`, prompt context, and check-ins.
- Let prune drop a stale registration whose worktree directory is gone. Run remote `doctor` and `prune` under the setup time limit.

## 0.6.2 - 2026-10-14

- Accept a host checkout whose origin names the same repository in another spelling, such as `git@github.com:owner/repo.git` for `https://github.com/owner/repo.git`. Compare host and repository path, not the raw URL.

## 0.6.1 - 2026-10-14

- Remove the hourly automatic worktree cleanup from `serve`. Wrangle never removes anything on a timer.
- Prompt the owner as soon as its child finishes, with the exact `wrangle close` command. Idle and done count as one event.
- List close commands in prompt context only for the caller's own finished children. `status --brief` shows only the count.
- Stop prompting about an agent item after it closes.

## 0.6.0 - 2026-10-14

- Add `close PANE [--host H]`: close a finished child's Herdr workspace, remove its worktree with `wt remove` so the repository's remove hooks run, and end its item. Refuse active agents, dirty or foreign-locked worktrees, and the caller's own workspace before any change.
- Keep an idle or done agent item open until its pane closes. Check-ins name the exact `wrangle close` command, and the brief, status, and Pi widget show unclosed children.

## 0.5.0 - 2026-10-13

- Detect transcript stalls, publish stall timestamps, and set or clear sidebar marks. Include stall counts in prompt context.
- Add durable idle-owner check-ins, job-completion events, and top-level stall notifications, independent from admission and publication.
- Resolve exact Claude and Codex transcript session IDs without choosing an ambiguous file.
- Add read-only `doctor`, confirmed `prune`, and hourly safe worktree cleanup in registered repositories.
- Revalidate complete Git identities and remove only the approved path. Keep repository registrations after the last worktree removal.
- Preserve cleanup reports when registered paths are missing or unreadable. Never kill reported processes or force-remove worktrees.
- Stop observing closed items. Show sanitized transcript activity rather than agent footer lines in check-ins.

- Package wrangle hooks and one shared skill for Pi, Claude Code, and Codex. Add Claude and Codex marketplaces.
- Add `hook context --harness claude|codex|pi`, with event-specific JSON or plain text from the shared brief formatter.
- Bound prompt context to five seconds. Missing Herdr, snapshots, or binaries do not block prompts.
- Keep package versions aligned with Cargo and verify plugin paths and release service templates in tests.

- Keep job capture and exit status independent from a closed pane output.
- Enforce one command deadline across discovery, preparation, transport, and lock waits. Bound routine operations to 30 seconds and setup operations to 10 minutes.
- Run service actions independently from snapshot publication and record ticket timeouts without stopping the probe loop.
- Keep the Pi polling timer active after an initial refresh failure. Race concurrent store tests through barriers.

- Add jobs (`run`, `exec`, `ps`, `logs`, `stop`, `wait`) with output logs, process-group cancellation, deadlines, and sidebar lifecycle reports.
- Add harness-independent `agent`, owner-host `track` and `items`, and locked FIFO admission with `cancel` and idle-owner wake-ups.
- Add jobs to host snapshots and open items and tickets to status. Service admission and owner reconciliation use the peer cache.
- Clone missing repository checkouts from the caller's origin before prepare or launch. Reject origin conflicts without overwriting paths.
- Check mise trust after fresh clones and trust only the new checkout when mise refuses its configuration.
- Track Shepherdr spawns in Pi and show open jobs and tickets above the editor, refreshed every ten seconds and after tool results.
- Add `serve` with local-only probes, atomic `host.json`, memory used/total, and a configurable ten-second interval.
- Record each agent pane, harness, state, transcript path, and transcript file modification time in the host snapshot.
- Make `status` read snapshots, with parallel peer reads and a shared ten-second cache. Show stale and unavailable hosts instead of hiding them.
- Add `status --line`, `status --brief`, and `place`. Placement reserves capacity until its TTL (90 seconds by default) and can prepare a worktree.
- Run host-policy prepare hooks in the repository checkout. Restore `--base` and literal `{branch}`/`{base}` substitution. Reject unknown placeholders. Remove repository hook overrides.
- Report Linux working-set memory from the cgroup with the smallest finite ancestor limit.
- Bundle launchd and systemd service definitions in release archives. Dotfiles installs them later.
- Replace the Pi spawn tool with CLI placement and keep prompt context from `wrangle status --brief`.
- Remove leads, marks, the old spawn/ticket API, lifecycle reports, and Herdr release hooks. The new stores do not migrate old ticket state.
- Remove the sidebar snippet and obsolete lead/placement plan. Keep the Herdr status-board popup.
- Document the approved transcript-based stalls, owner check-ins, and cleanup decisions for later units.

This entry does not change package versions or publish a release.

## 0.4.0 - 2026-10-09

- Repository config: an optional `wrangle.toml` at the root of a repository sets
  `[hooks] prepare` for spawns from that repository. It replaces the host policy's hook. The lead
  reads it from the main checkout. Admission thresholds stay in `~/.config/wrangle/config.toml`.
- The `prepare` hook's `{repo}` is now valid on the admitted host. A directory resolves to its
  main checkout, so a lead inside a linked worktree works. On a remote host the checkout is
  relative to home, where ssh starts the hook. Hooks no longer need to rebuild the path from a
  fixed folder such as `~/dev/<name>`.

## 0.3.0 - 2026-10-09

- Lifecycle log: the ledger appends `queued`, `admitted`, `spawned`, `released`, and `cancelled`
  events to `events.jsonl` under its lock. It stores no prompts or messages, keeps 30 days, and
  skips a torn line instead of failing.
- New command `wrangle report [--since <duration>] [--json]`: per-ticket queue wait and run time,
  plus host refusal counts with reasons.
- `wrangle status` shows the refusal reason for every ineligible host.
- The Pi tool passes `base` only to the `prepare` hook, never to Shepherdr's spawn.

## 0.2.1 - 2026-10-09

- The Pi tool never blocks: every spawn goes to `agents spawn` with `blocking: false`, so the call
  returns once the child has its task and the lead keeps working. The child's completion arrives
  as a Shepherdr message. The `blocking` parameter is removed.

## 0.2.0 - 2026-10-08

- One lead per repository. The first spawn from a Herdr pane claims the lead for the pane's
  repository (`⌂ <repo>`, tokens `role=lead`, `repo=`). A second live lead is refused with
  `lead_exists` before anything is admitted; `take_over=true` / `--take-over` moves it.
  New command `wrangle lead`.
- Children carry `lead=lead:<repo>` instead of `owner=`. `wrangle status` lists every lead with
  its children before the hosts; older `owner=` children are grouped too.
- Layout is no longer a choice: the Pi tool drops `placement`, `workspace`, and `pane`, and the
  CLI drops `--workspace`. A child with a branch opens in its worktree; any other child opens as
  a tab in the lead's workspace (a new workspace on a remote host). `spawn --lead` is optional
  inside Herdr.
- Pi widget shows the session's lead, its children by status, the queue, and host headroom;
  refreshes on every call and every 60 s while work runs. Toasts on spawn and on `lead_exists`.
- `wrangle status --sidebar` prints the Herdr sidebar rows (`herdr-sidebar.toml`).

## 0.1.1 - 2026-10-08

- Herdr plugin: `pane.closed` and `pane.exited` release the pane's reservation; a `board`
  popup runs `wrangle status --watch`.

## 0.1.0 - 2026-10-08

- Skill `wrangle`: the orchestration procedure for Pi and for shell harnesses.
- README: install, configuration, CLI, Pi tool, sidebar rows, skill.

- Pi tool `wrangle`: `spawn` (default) admits on the host with the most headroom, runs the
  `prepare` hook when `branch` is set, calls Shepherdr `agents spawn` on that host, and marks
  the child pane; `status`, `cancel`, `help`. Queued calls return a ticket at once.
- Queue: tickets persist in the session (`wrangle-queue` entry), a poll timer (30 s → 60 s)
  re-admits the oldest ticket, admission sends a `wrangle` wake-up message, `wrangle ticket=…`
  reuses the stored arguments; queued tickets show in a widget above the editor, as a
  `queue` token on the lead's Herdr pane, and as a toast.
- CLI `spawn` for harnesses that are not Pi (Claude Code, Codex, …): admit or resume a ticket,
  place a pane (the `prepare` hook with `--branch`, else a new tab or workspace), `herdr agent
  start`, mark, `herdr agent prompt`. An agent blocked on a startup dialog keeps its pane and
  returns `status: blocked` with the next step.
- Engine: `prepare` command runs the policy's `[hooks] prepare` template on a host.
- Engine: `probe`, `admit`, `release`, `status`, `cancel`, `mark`, `queue`, `notify`, and the
  per-host `host` primitives. Balanced admission by headroom, TTL reservations under a
  per-host lock, queue tickets, remote hosts over `ssh -T` (`remote_command`, default
  `mise x -- wrangle`).
- Scaffold: Cargo crate, Pi package manifest, CI, cargo-dist release workflow.
