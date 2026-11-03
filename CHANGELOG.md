# Changelog

## 0.7.1 (2026-11-03)

### Other changes

- set the updater public key ([efa944e](https://github.com/openledger-nl/ledger/commit/efa944e58ea5fe5dad850c98a31760265beaf814))
- sign updates with a key that has no password ([dafb6be](https://github.com/openledger-nl/ledger/commit/dafb6be5869d60b722f0473487b2e0221a0dfd88))

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
