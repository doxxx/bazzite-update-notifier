# Changelog

## [0.2.1](https://github.com/doxxx/bazzite-update-notifier/compare/bazzite-update-notifier-v0.2.0...bazzite-update-notifier-v0.2.1) (2026-06-03)


### Bug Fixes

* **ci:** use version output to avoid duplicate name in release tarball ([9629a06](https://github.com/doxxx/bazzite-update-notifier/commit/9629a063ad4d9756dcdadefdba017a34b5c247f2))

## [0.2.0](https://github.com/doxxx/bazzite-update-notifier/compare/bazzite-update-notifier-v0.1.0...bazzite-update-notifier-v0.2.0) (2026-06-03)


### Features

* initial implementation by Claude ([f28336d](https://github.com/doxxx/bazzite-update-notifier/commit/f28336d9ddf9c0702415b5f854e9a01701d3e3a8))
* **tray:** indicate when a pending deployment is staged and ready for reboot ([bbb882e](https://github.com/doxxx/bazzite-update-notifier/commit/bbb882e282f45f5cb4bba79944b4881f1bdc1058))


### Bug Fixes

* **install:** install.sh must create cache dir ([5390660](https://github.com/doxxx/bazzite-update-notifier/commit/5390660e63739c9e96230b8619e473b22202df12))
* **logging:** add and improve logging of actions taken ([7105f32](https://github.com/doxxx/bazzite-update-notifier/commit/7105f324206b97fd2419914f5e396563a840fe94))
* **logging:** only log warnings or higher from zbus/D-Bus ([48e61cb](https://github.com/doxxx/bazzite-update-notifier/commit/48e61cb774cbcc36ad1f14519e57bb78ebeb3843))
* **resolver:** use correct GitHub API response format for releases ([a34f114](https://github.com/doxxx/bazzite-update-notifier/commit/a34f11434bd3501e3ad3858d716eb1ef820d65a8))
* **systemd:** remove ProtectHome and ReadWritePaths from service file ([287a26f](https://github.com/doxxx/bazzite-update-notifier/commit/287a26f6f7badd60d9932f564b1182fdee00311b))
* **tray:** periodically update tooltip to indicate time since last check ([0eeda35](https://github.com/doxxx/bazzite-update-notifier/commit/0eeda35977a3e8c7b9b8fc0231e7590b4eeced2e))
* **tray:** properly handle tray icon events during 60s startup delay ([3867a9c](https://github.com/doxxx/bazzite-update-notifier/commit/3867a9c0a3c4c3f278c68b08d3b43f39d65bf055))
