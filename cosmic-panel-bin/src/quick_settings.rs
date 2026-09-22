// Copyright 2026 System76 <info@system76.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Panel-side Quick Settings composition.
//!
//! The panel owns presentation and layout. Applets remain authoritative for
//! their state and are identified alongside each model so user actions can be
//! routed back to the process that owns the control.

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::Sender;

use cosmic::app::quick_settings::{
    QuickSettingControl, QuickSettingKind, QuickSettingsAction, QuickSettingsEvent,
    QuickSettingsModel,
};
use cosmic::iced::{Alignment, Length};
use cosmic::iced::widget::{row, slider};
use cosmic::widget::{button, column, container, dropdown, grid, icon, text, toggler};

use crate::iced::{Element, IcedProgram};

/// Environment variable containing the inherited Quick Settings capability FD.
pub const QUICK_SETTINGS_FD_ENV: &str = "COSMIC_QUICK_SETTINGS";

/// Create the private capability channel used between cosmic-panel and one applet.
///
/// The host endpoint stays in cosmic-panel. The child endpoint is inherited by the applet
/// process and later consumed by libcosmic's transport layer.
pub fn quick_settings_channel() -> std::io::Result<(UnixStream, OwnedFd)> {
    let (host, child) = UnixStream::pair()?;
    host.set_nonblocking(true)?;
    child.set_nonblocking(true)?;
    Ok((host, child.into()))
}

/// A Quick Settings model together with the applet process that owns it.
#[derive(Debug, Clone, PartialEq)]
pub struct AppletQuickSettings {
    pub applet_id: String,
    pub model: QuickSettingsModel,
}

/// Event emitted by the panel renderer and ready to be sent to one applet.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutedQuickSettingsEvent {
    pub applet_id: String,
    pub event: QuickSettingsEvent,
}

/// Host-side program that composes controls from multiple applets.
#[derive(Debug, Clone)]
pub struct QuickSettingsProgram {
    sources: Vec<AppletQuickSettings>,
    event_tx: Sender<RoutedQuickSettingsEvent>,
}

impl QuickSettingsProgram {
    pub fn new(
        sources: Vec<AppletQuickSettings>,
        event_tx: Sender<RoutedQuickSettingsEvent>,
    ) -> Self {
        Self { sources, event_tx }
    }

    pub fn set_sources(&mut self, sources: Vec<AppletQuickSettings>) {
        self.sources = sources;
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Action(RoutedQuickSettingsEvent),
}

impl IcedProgram for QuickSettingsProgram {
    type Message = Message;

    fn update(&mut self, message: Self::Message) -> cosmic::iced::Task<Self::Message> {
        match message {
            Message::Action(event) => {
                let _ = self.event_tx.send(event);
            }
        }

        cosmic::iced::Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let mut layout = grid()
            .column_spacing(8)
            .row_spacing(8)
            .width(Length::Fill);

        let mut column = 1_u16;
        let mut row = 1_u16;

        for source in &self.sources {
            for control in &source.model.controls {
                let span = span_for(control);
                if column + span - 1 > 4 {
                    row += 1;
                    column = 1;
                }

                let applet_id = source.applet_id.clone();
                let element = control_view(applet_id, control);

                let assignment = (column, row, span, 1);
                layout = layout.push_with(element, move |_| assignment.into());

                column += span;
                if column > 4 {
                    row += 1;
                    column = 1;
                }
            }
        }

        container(layout).padding(8).width(Length::Fill).into()
    }
}

/// Initial 4×N host policy.
///
/// This intentionally lives in cosmic-panel rather than the applet model.
fn span_for(control: &QuickSettingControl) -> u16 {
    match control.kind {
        QuickSettingKind::Button | QuickSettingKind::Toggle { .. } => 2,
        QuickSettingKind::Slider { .. } | QuickSettingKind::SingleSelect { .. } => 4,
    }
}

fn control_view<'a>(
    applet_id: String,
    control: &'a QuickSettingControl,
) -> Element<'a, Message> {
    match &control.kind {
        QuickSettingKind::Button => {
            let id = control.id.clone();
            button::custom(label_content(control))
                .on_press(Message::Action(routed(
                    applet_id,
                    id,
                    QuickSettingsAction::Activate,
                )))
                .width(Length::Fill)
                .into()
        }
        QuickSettingKind::Toggle { value } => {
            let id = control.id.clone();
            let applet_id = applet_id.clone();
            let toggle = toggler(*value)
                .label(control.label.clone())
                .width(Length::Fill)
                .on_toggle(move |value| {
                    Message::Action(routed(
                        applet_id.clone(),
                        id.clone(),
                        QuickSettingsAction::SetToggle(value),
                    ))
                });

            container(toggle).padding(8).width(Length::Fill).into()
        }
        QuickSettingKind::Slider {
            value,
            min,
            max,
            breakpoints,
        } => {
            let id_change = control.id.clone();
            let id_release = control.id.clone();
            let applet_change = applet_id.clone();
            let applet_release = applet_id;

            let mut control_slider = slider(*min..=*max, *value, move |value| {
                Message::Action(routed(
                    applet_change.clone(),
                    id_change.clone(),
                    QuickSettingsAction::SetValue(value),
                ))
            })
            .width(Length::Fill)
            .on_release(Message::Action(routed(
                applet_release,
                id_release,
                QuickSettingsAction::Release,
            )));

            if let Some(breakpoints) = breakpoints {
                control_slider = control_slider.breakpoints(breakpoints.as_slice());
            }

            let content = row![
                leading_icon(control),
                column![
                    text::body(control.label.clone()),
                    control_slider,
                ]
                .spacing(4)
                .width(Length::Fill),
                control
                    .secondary
                    .as_ref()
                    .map(|secondary| text::caption(secondary.clone()).into())
                    .unwrap_or_else(|| cosmic::widget::space::horizontal().width(0).into()),
            ]
            .spacing(8)
            .align_y(Alignment::Center);

            container(content).padding(8).width(Length::Fill).into()
        }
        QuickSettingKind::SingleSelect { selected, options } => {
            let labels = options.iter().map(|option| option.label.clone()).collect::<Vec<_>>();
            let option_ids = options.iter().map(|option| option.id.clone()).collect::<Vec<_>>();
            let selected = selected
                .as_ref()
                .and_then(|selected| options.iter().position(|option| &option.id == selected));
            let id = control.id.clone();

            let select = dropdown(labels, selected, move |index| {
                Message::Action(routed(
                    applet_id.clone(),
                    id.clone(),
                    QuickSettingsAction::Select(option_ids[index].clone()),
                ))
            })
            .width(Length::Fill);

            let content = column![
                row![
                    leading_icon(control),
                    column![
                        text::body(control.label.clone()),
                        control
                            .secondary
                            .as_ref()
                            .map(|secondary| text::caption(secondary.clone()).into())
                            .unwrap_or_else(|| cosmic::widget::space::horizontal().width(0).into()),
                    ]
                    .width(Length::Fill),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                select,
            ]
            .spacing(4);

            container(content).padding(8).width(Length::Fill).into()
        }
    }
}

fn routed(
    applet_id: String,
    id: String,
    action: QuickSettingsAction,
) -> RoutedQuickSettingsEvent {
    RoutedQuickSettingsEvent {
        applet_id,
        event: QuickSettingsEvent { id, action },
    }
}

fn label_content<'a>(control: &'a QuickSettingControl) -> Element<'a, Message> {
    row![
        leading_icon(control),
        column![
            text::body(control.label.clone()),
            control
                .secondary
                .as_ref()
                .map(|secondary| text::caption(secondary.clone()).into())
                .unwrap_or_else(|| cosmic::widget::space::horizontal().width(0).into()),
        ]
        .width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

fn leading_icon<'a>(control: &'a QuickSettingControl) -> Element<'a, Message> {
    control
        .icon
        .as_ref()
        .map(|name| icon::from_name(name.clone()).size(20).symbolic(true).into())
        .unwrap_or_else(|| cosmic::widget::space::horizontal().width(0).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control(kind: QuickSettingKind) -> QuickSettingControl {
        QuickSettingControl {
            id: "test".into(),
            label: "Test".into(),
            secondary: None,
            icon: None,
            kind,
        }
    }

    #[test]
    fn compact_controls_use_half_row() {
        assert_eq!(span_for(&control(QuickSettingKind::Button)), 2);
        assert_eq!(span_for(&control(QuickSettingKind::Toggle { value: true })), 2);
    }

    #[test]
    fn value_controls_use_full_row() {
        assert_eq!(
            span_for(&control(QuickSettingKind::Slider {
                value: 0.5,
                min: 0.0,
                max: 1.0,
                breakpoints: None,
            })),
            4
        );
        assert_eq!(
            span_for(&control(QuickSettingKind::SingleSelect {
                selected: None,
                options: Vec::new(),
            })),
            4
        );
    }
}
