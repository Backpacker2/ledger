# Changelog

## 0.8.0 (2026-11-03)

### Features

- **mobile:** deploy the server to a linux box too ([c9caf0c](https://github.com/Backpacker2/ledger/commit/c9caf0cf06820f897128caa7a1a5492951f785ce))
- **mobile:** deploy the server to a mac at home ([30aec0b](https://github.com/Backpacker2/ledger/commit/30aec0b40484146492f5017b0cddf3220f7afa60))
- **mobile:** add ledger to a phone's home screen ([eac47e6](https://github.com/Backpacker2/ledger/commit/eac47e6dde5a6ddc1fd99bbbff61d368845bb365))
- **mobile:** fit the screens to a phone ([cb7c6ae](https://github.com/Backpacker2/ledger/commit/cb7c6aea9a83c5d25b3818be957b9e7968da1358))
- **mobile:** run the screens in a browser against ledger-server ([7a55d2d](https://github.com/Backpacker2/ledger/commit/7a55d2df0ed33e5d3e5a6cdd4ac6dffd15908a6f))
- **mobile:** add ledger-server, the commands over http for a phone ([445e25a](https://github.com/Backpacker2/ledger/commit/445e25aa75e74bf98b5b017ea6f21f619c531edf))

### Fixes

- **logs:** report problems to backpacker2/ledger ([8eb9a61](https://github.com/Backpacker2/ledger/commit/8eb9a6111663dd2c871e81c1c6e90e9fe493c061))
- **mobile:** keep the server's own config when redeploying ([357d96a](https://github.com/Backpacker2/ledger/commit/357d96a2f946b63117358ad9616a0ee81e4fa359))
- **mobile:** let the whole tailnet in when the login list is empty ([69f6f44](https://github.com/Backpacker2/ledger/commit/69f6f444c59c46e268de9e68f947b9592377d27a))

### Other changes

- let a release be started by hand ([cca10cb](https://github.com/Backpacker2/ledger/commit/cca10cb4c0c1bb20f24fc13a1ee0bb1bd3360fb2))
- add the updates scope to the commit scopes ([3ec8473](https://github.com/Backpacker2/ledger/commit/3ec847307ed06384659e2d4c67ffb80652d55288))
- link the releases of backpacker2/ledger ([f5334e3](https://github.com/Backpacker2/ledger/commit/f5334e35a327a7cebc3167d0a225d06c59e3e3eb))
- **updates:** sign releases with this ledger's own key ([5c5c539](https://github.com/Backpacker2/ledger/commit/5c5c539f36b2c3e4f31c5de35bdb0a4b3711dc94))
- **mobile:** build ledger-server without tauri ([097eb52](https://github.com/Backpacker2/ledger/commit/097eb52bfd06f1345077034da8de176ef96fa695))
- **mobile:** test the server feature too ([0cb0d11](https://github.com/Backpacker2/ledger/commit/0cb0d118cfcbbd5bb60aafb4904d245e5ed64e59))
- **mobile:** let the commands run without the desktop window ([8f1e852](https://github.com/Backpacker2/ledger/commit/8f1e852df149e61b5dfb10c732b93b0ebf13ad10))

## 0.7.0 (2026-11-03)

### Features

- **settings:** show what Ledger is and where it keeps things in the About tab ([b09b75a](https://github.com/openledger-nl/ledger/commit/b09b75ab2eebbea3fa93f18f6a5cc84272aa29bf))
- **logs:** log errors on this machine and report them from a new Logs tab ([b701f71](https://github.com/openledger-nl/ledger/commit/b701f711ccd15e2276bad1887d3607c1017812b4))
- **settings:** group the settings into tabs, with a search and on/off switches ([49edeb3](https://github.com/openledger-nl/ledger/commit/49edeb3681319411d2bcf2eced6445ebb67ea31a))

### Other changes

- change the app identifier to nl.openledger.ledger ([c6c1e3b](https://github.com/openledger-nl/ledger/commit/c6c1e3bffb3fa8b331d925d9849e33dea060d057))

## 0.6.0 (2026-11-03)

### Features

- **updates:** install new releases from inside the app ([485fd0e](https://github.com/openledger-nl/ledger/commit/485fd0ef02dcb863dbf0789a7908724088254ada))
- **deliverables:** switch between source, live preview and reading, as in Obsidian ([d9a19d1](https://github.com/openledger-nl/ledger/commit/d9a19d18a0b6978b9b8cef80f94f42cdcc0008e7))
- **deliverables:** write a version's changelog in the markdown editor ([c9132c8](https://github.com/openledger-nl/ledger/commit/c9132c8b1a948b96eb36cb67353799f2fbf1ede8))
- **ui:** write markdown in an inline CodeMirror editor everywhere ([8adcbf4](https://github.com/openledger-nl/ledger/commit/8adcbf4ceade87e554cec49adaf15419c02a325f))

### Fixes

- **ui:** draw tables in the live preview and show pictures from the vault ([4d03d83](https://github.com/openledger-nl/ledger/commit/4d03d83a186f15714329ad32af760647ed1df811))

### Other changes

- push the release commit with a deploy key so the main ruleset lets it through ([e81017a](https://github.com/openledger-nl/ledger/commit/e81017a132cbd737de9f4d1c7f4cf43eb4240d93))
- check that every commit in a pull request is conventional ([e2e7df8](https://github.com/openledger-nl/ledger/commit/e2e7df8ada8581e907e3c23fdffd3565378e8f22))
- add a contributing guide ([a548042](https://github.com/openledger-nl/ledger/commit/a548042ec8a72ed5b4ca818e8f96ae30de4e6382))

## 0.5.0 (2026-11-03)

### Features

- contribution graph and dashboard update ([5a72938](https://github.com/openledger-nl/ledger/commit/5a72938ba1adba6e6a90d8572bd743b7e83aabc3))

## 0.4.0 (2026-11-03)

### Features

- **deliverables:** quote the changelog in the version published note ([0a4fa99](https://github.com/openledger-nl/ledger/commit/0a4fa998de2f8df69bec8edf39db8a234618b669))
- **settings:** one global order for posts, threads, feedback and versions ([6dd7e32](https://github.com/openledger-nl/ledger/commit/6dd7e32bbd735555abd2e12524ec1bed90b93fe4))
- **claude:** add a setting to run a prompt as soon as it is created ([b697886](https://github.com/openledger-nl/ledger/commit/b697886d6719ab2a939bdee25f8594b43145d1e3))
- **deliverables:** show how a deliverable was produced in a window beside it ([e7fa06a](https://github.com/openledger-nl/ledger/commit/e7fa06a7e6805623f90d34989bc3bd6934efe905))
- **deliverables:** note the prompt behind a version and show labels in the list ([2cb550a](https://github.com/openledger-nl/ledger/commit/2cb550a4b63292bbdcc9dcdb1ac4d34bef481669))
- **deliverables:** record how each published version came to be ([123eff6](https://github.com/openledger-nl/ledger/commit/123eff6776a3b3d6a4d3202a924133815c6b37b3))

### Fixes

- **deliverables:** do not guess who made kept edits, and keep a run with its own prompt ([37ce47a](https://github.com/openledger-nl/ledger/commit/37ce47a18c32cbd06a18b4111ddef6fae332fdb4))

### Other changes

- **deliverables:** use Lucide icons and colour tokens in the produced-by window ([6e983ec](https://github.com/openledger-nl/ledger/commit/6e983ecf38fec764c7d3b9549654778138904905))

## 0.3.0 (2026-11-03)

### Features

- **brand:** let the logo take the theme's accent ([9b66b95](https://github.com/openledger-nl/ledger/commit/9b66b95d621994cfdc8ee3adcd5f040db02be32d))

### Other changes

- list the commit types and scopes in CLAUDE.md ([50a0cab](https://github.com/openledger-nl/ledger/commit/50a0cab9a33984f8adf60c40611bdffd37e58955))
- typecheck and test every push and pull request ([5cc5c40](https://github.com/openledger-nl/ledger/commit/5cc5c40f867c156bfdb22255b40ba54c46bfe31c))
- move to Node 26 ([87fb3dd](https://github.com/openledger-nl/ledger/commit/87fb3dd0b35ce2423de5006938979a035a560fec))

## 0.2.0 (2026-11-03)

### Features

- **threads:** let the chat use the full width ([5c87405](https://github.com/openledger-nl/ledger/commit/5c874055ec817dc57870a504789ba6c3aa43622a))
- **ui:** use Lucide icons instead of text glyphs ([91762d9](https://github.com/openledger-nl/ledger/commit/91762d9eddcd18644396b8ccf8213bce6aa57c7d))
- **screenshots:** bring your own screenshot tool, such as Flameshot ([2534690](https://github.com/openledger-nl/ledger/commit/2534690730be5a10e58c0341675c7151c2680e97))

### Other changes

- release every push to main from its conventional commits ([7a8f96a](https://github.com/openledger-nl/ledger/commit/7a8f96a1272a167f0515d4137ec6cd46c81422ff))
