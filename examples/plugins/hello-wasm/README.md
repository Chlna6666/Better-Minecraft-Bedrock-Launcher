# BMCBL Essentials

BMCBL Essentials is the reference plugin for the BMCBL WASM Component plugin system.

It demonstrates:

- page registration and navigation
- safe UI injection
- host toast and plugin window APIs
- global event subscription
- plugin i18n through `.lang` files
- read-only plugin config access from WASM
- user-editable config rendered by the BMCBL plugin settings page

The plugin runs inside the Wasmtime sandbox. It does not receive filesystem, network, or process execution access.
