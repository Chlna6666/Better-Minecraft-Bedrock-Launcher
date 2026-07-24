# egpui-build

`egpui-build` contains deterministic build-time contracts for egpui
applications. It loads and validates `App.toml`, indexes declared resources,
and produces platform bundle plans for a native toolchain to execute.

The crate does not run downloads, archives, task-manager workflows, or
platform packaging tools. Application-specific build policy remains in the
application crate.
