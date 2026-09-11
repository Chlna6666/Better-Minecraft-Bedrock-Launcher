use super::super::*;

impl MainWindowView {
    pub(super) fn ensure_settings_github_control(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.update_global(
            |state: &mut crate::ui::views::settings::state::SettingsPageState, cx| {
                if state.download_github_custom_mirror_input.is_none() {
                    let initial = state.download_github_custom_mirror.to_string();
                    let input = cx.new(|cx| {
                        let mut input = InputState::new(window, cx);
                        input.set_placeholder(
                            SharedString::from("https://mirror.example/{url}"),
                            window,
                            cx,
                        );
                        if !initial.trim().is_empty() {
                            input.set_value(SharedString::from(initial), window, cx);
                        }
                        input
                    });
                    state.download_github_custom_mirror_input = Some(input);
                }
                state.download_github_custom_mirror_input.clone()
            },
        );

        if let Some(input) = input {
            let subscription =
                cx.subscribe(&input, |this, input, event: &InputEvent, cx| match event {
                    InputEvent::Change => {
                        let value = input.read(cx).value();
                        cx.update_global(
                            |state: &mut crate::ui::views::settings::state::SettingsPageState,
                             _cx| {
                                state.download_github_custom_mirror = value;
                            },
                        );
                        this.notify_settings_page(cx);
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. } => {
                        let custom_mirror = cx.read_global(
                            |state: &crate::ui::views::settings::state::SettingsPageState, _cx| {
                                state.download_github_custom_mirror.to_string()
                            },
                        );
                        persist_custom_mirror(custom_mirror, cx);
                    }
                    _ => {}
                });
            self.settings_controls_subscriptions.push(subscription);
        }
    }
}

fn persist_custom_mirror(custom_mirror: String, cx: &mut Context<MainWindowView>) {
    cx.spawn(async move |_this, _cx| {
        let result = crate::tasks::runtime::run_io_blocking(move || {
            crate::config::config::update_config(|config| {
                config.launcher.download.github.custom_mirror = custom_mirror;
            })?;
            Ok::<(), std::io::Error>(())
        })
        .await;

        match result {
            Err(error) => tracing::warn!("persist GitHub mirror join error: {error}"),
            Ok(Err(error)) => tracing::warn!("persist GitHub mirror failed: {error}"),
            Ok(Ok(())) => {}
        }
    })
    .detach();
}
