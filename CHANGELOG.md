# Changelog

## [0.2.0](https://github.com/doxxx/bazzite-update-notifier/compare/bazzite-update-notifier-v0.1.0...bazzite-update-notifier-v0.2.0) (2026-06-03)


### Features

* **tray:** indicate when a pending deployment is staged and ready for reboot ([0d53f78](https://github.com/doxxx/bazzite-update-notifier/commit/0d53f78091dd14cbff2c2d9823806a760b6a2ed7))


### Bug Fixes

* **install:** install.sh must create cache dir ([200797e](https://github.com/doxxx/bazzite-update-notifier/commit/200797e4dd6d682c7761a9018901ca64f0386ca8))
* **logging:** add and improve logging of actions taken ([df399de](https://github.com/doxxx/bazzite-update-notifier/commit/df399de4d39f33cc44d505adaca94c3fc2dc9dd8))
* **logging:** only log warnings or higher from zbus/D-Bus ([7b3c4d8](https://github.com/doxxx/bazzite-update-notifier/commit/7b3c4d89985d30eaea7cedefa3de95c4b03c984c))
* **resolver:** use correct GitHub API response format for releases ([be284d3](https://github.com/doxxx/bazzite-update-notifier/commit/be284d31966ee8853a33e9e0336eb7ace6b2f3ec))
* **systemd:** remove ProtectHome and ReadWritePaths from service file ([d79c113](https://github.com/doxxx/bazzite-update-notifier/commit/d79c1136fd1e9259538381260dde587965fdb24c))
* **tray:** periodically update tooltip to indicate time since last check ([9294693](https://github.com/doxxx/bazzite-update-notifier/commit/92946932ce7c7a9dcc888d5364a76dab29fde466))
* **tray:** properly handle tray icon events during 60s startup delay ([b8f4d33](https://github.com/doxxx/bazzite-update-notifier/commit/b8f4d33464cdf63ca11777bc8165a7798d48d227))
