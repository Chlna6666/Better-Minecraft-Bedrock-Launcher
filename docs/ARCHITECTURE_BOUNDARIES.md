# Architecture Boundaries

## GPUI Framework Code

GPUI framework code owns platform windows, renderer backends, frame scheduling,
scene preparation, text systems, and generic UI primitives. It must not depend
on BMCBL routes, pages, asset names, default backgrounds, download services,
launcher state, or application window policy.

Framework defaults should be portable and conservative. Low idle work,
event-driven rendering, and adapter/present-mode options belong here because
they define how GPUI behaves for every application using it.

## BMCBL Application Code

Application defaults belong in `src/app.rs` and `src/ui`. This includes the
preferred renderer options for BMCBL, embedded fonts, default backgrounds,
main-window chrome, startup services, background image policy, and launcher
workflow state.

Render methods and generic components in `src/ui` should coordinate UI state
only. Network IO, persistent cache storage, decode pipelines, downloads, and
durable program workflows should stay outside render methods.

## Change Rule

Before changing GPUI framework code, verify whether the behavior is
framework-wide or BMCBL-specific. If the behavior references BMCBL assets,
routes, downloads, launcher policy, or background selection, keep it outside the
framework.
