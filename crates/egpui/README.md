# egpui

`egpui` is the desktop application layer built on top of the independently
maintained `gpui` GUI core.

It provides:

- `ApplicationHost` lifecycle, headless mode, service registration, and
  deadline-based shutdown;
- an application-owned Tokio/Rayon runtime behind `RuntimeProvider`;
- structured `TaskScope` cancellation and explicit `TaskOutcome` terminal states;
- bounded `UiHandle`, `UiStreamBridge<T>`, and coalescing snapshot channels for
  background-to-GPUI foreground updates;
- `UiTaskBridge` for coalesced high-frequency progress and a dedicated,
  lossless terminal update, including cancellation-on-drop behavior;
- framework-owned Fluent i18n catalogs with BCP 47 locale fallback, plural
  formatting, RTL direction metadata, and watch-based locale snapshots;
- generic durable task records and atomic checkpoint persistence. Application
  handlers decide how downloads or archives resume; egpui does not own those
  domains.
- `DurableWorkflowCoordinator` for handler registration, restart recovery, and
  terminal checkpoint persistence without taking ownership of product workflows.

The crate deliberately does not contain BMCBL routes, download/archive
semantics, or task-manager policy. It owns generic resources, localization
runtime, and cross-platform bundle planning, while applications register their
catalogs and domain services through `ServiceRegistry`.

The i18n layer is intentionally domain-neutral: message keys, catalog files,
and product copy remain application-owned. egpui provides the runtime
registration, fallback, formatting, direction, and UI snapshot contract.

The public API does not require callers to construct or detect a Tokio runtime.
GPUI foreground work remains separate and must use GPUI's `App`/`AsyncApp`
execution domain.

For downloads, archive extraction, or other large operations, schedule the
producer through `TaskScope` and use `UiTaskBridge` only for pure progress and
terminal data. The bridge never performs filesystem, network, decoding, or
blocking work on the GPUI foreground executor.

Backend-to-GPUI data flow has three explicit choices:

- `UiHandle` for short, ordered state mutations with bounded backpressure;
- `UiStreamBridge<T>` for ordered channel-backed events where every event must
  be observed;
- `UiTaskBridge` for latest progress plus one lossless terminal outcome.

All three APIs apply data on GPUI's foreground executor. Producers send owned
`Send + 'static` snapshots or events and never retain GPUI handles.
