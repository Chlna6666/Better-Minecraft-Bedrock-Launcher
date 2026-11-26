//! Provides a [calloop] event source from [XDG Desktop Portal] events
//!
//! This module uses the [ashpd] crate

use ashpd::desktop::settings::{ColorScheme, Settings};
use calloop::channel::Channel;
use calloop::{EventSource, Poll, PostAction, Readiness, Token, TokenFactory};
use smol::stream::StreamExt;

use std::{future::Future, pin::Pin};

/// An XDG settings task to schedule on the application's background executor.
pub type SettingsTask = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;

pub enum Event {
    PreferDark(bool),
    CursorTheme(String),
    CursorSize(u32),
}

pub struct XdpEventSource {
    channel: Channel<Event>,
}

impl XdpEventSource {
    /// Subscribes to desktop settings using the application's background scheduler.
    pub fn new(spawn: impl Fn(SettingsTask) + Clone + Send + 'static) -> Self {
        let (sender, channel) = calloop::channel::channel();
        let background = spawn.clone();
        spawn(Box::pin(async move {
            let settings = Settings::new().await?;

            if let Ok(initial_appearance) = settings.color_scheme().await {
                sender.send(Event::PreferDark(
                    initial_appearance == ColorScheme::PreferDark,
                ))?;
            }
            if let Ok(initial_theme) = settings
                .read::<String>("org.gnome.desktop.interface", "cursor-theme")
                .await
            {
                sender.send(Event::CursorTheme(initial_theme))?;
            }

            // If u32 is used here, it throws invalid type error
            if let Ok(initial_size) = settings
                .read::<i32>("org.gnome.desktop.interface", "cursor-size")
                .await
            {
                sender.send(Event::CursorSize(initial_size as u32))?;
            }

            if let Ok(mut cursor_theme_changed) = settings
                .receive_setting_changed_with_args("org.gnome.desktop.interface", "cursor-theme")
                .await
            {
                let sender = sender.clone();
                background(Box::pin(async move {
                    while let Some(theme) = cursor_theme_changed.next().await {
                        let theme = theme?;
                        sender.send(Event::CursorTheme(theme))?;
                    }
                    anyhow::Ok(())
                }));
            }

            if let Ok(mut cursor_size_changed) = settings
                .receive_setting_changed_with_args::<i32>(
                    "org.gnome.desktop.interface",
                    "cursor-size",
                )
                .await
            {
                let sender = sender.clone();
                background(Box::pin(async move {
                    while let Some(size) = cursor_size_changed.next().await {
                        let size = size?;
                        sender.send(Event::CursorSize(size as u32))?;
                    }
                    anyhow::Ok(())
                }));
            }

            let mut appearance_changed = settings.receive_color_scheme_changed().await?;
            while let Some(scheme) = appearance_changed.next().await {
                sender.send(Event::PreferDark(scheme == ColorScheme::PreferDark))?;
            }

            anyhow::Ok(())
        }));

        Self { channel }
    }
}

impl EventSource for XdpEventSource {
    type Event = Event;
    type Metadata = ();
    type Ret = ();
    type Error = anyhow::Error;

    fn process_events<F>(
        &mut self,
        readiness: Readiness,
        token: Token,
        mut callback: F,
    ) -> Result<PostAction, Self::Error>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        self.channel.process_events(readiness, token, |evt, _| {
            if let calloop::channel::Event::Msg(msg) = evt {
                (callback)(msg, &mut ())
            }
        })?;

        Ok(PostAction::Continue)
    }

    fn register(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut TokenFactory,
    ) -> calloop::Result<()> {
        self.channel.register(poll, token_factory)?;

        Ok(())
    }

    fn reregister(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut TokenFactory,
    ) -> calloop::Result<()> {
        self.channel.reregister(poll, token_factory)?;

        Ok(())
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.channel.unregister(poll)?;

        Ok(())
    }
}
