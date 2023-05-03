mod backend;
use std::net::SocketAddr;

use backend::Backend;
use cursive::{
    align::HAlign,
    theme::Effect,
    utils::span::SpannedString,
    view::{Nameable, Resizable, ScrollStrategy, SizeConstraint},
    views::{Dialog, EditView, LinearLayout, NamedView, ScrollView, TextView},
    Cursive,
};
use tokio::sync::mpsc::{error::TryRecvError, Receiver, Sender};

use crate::Str;

#[derive(Debug)]
pub enum UiEvent {
    UserConnected {
        at: SocketAddr,
        username: Option<Str>,
    },
    UserDisconnected {
        at: SocketAddr,
    },
    UpdateUserName {
        at: SocketAddr,
        new_username: Str,
    },
    NewMessage {
        at: SocketAddr,
        username: Option<Str>,
        message: Str,
    },
}

fn display_new_message(c: &mut Cursive, username: &str, message: &str) {
    c.call_on_name("messages", |messages: &mut LinearLayout| {
        let mut text = SpannedString::default();
        text.append_styled(username, Effect::Bold);
        text.append_plain(": ");
        text.append_plain(message);
        messages.add_child(TextView::new(text));
    });
}

pub fn ui(mut messages: Receiver<UiEvent>, send_message: Sender<Str>) {
    let mut cursive = Cursive::new();
    cursive.with_theme(|current| {
        use cursive::theme::{Color, PaletteColor};
        let color = Color::Rgb(93, 26, 20);
        current.palette[PaletteColor::Background] = color;
        current.palette[PaletteColor::Secondary] = color;
    });
    cursive.add_layer(
        Dialog::new()
            .title(SpannedString::styled("Presunto e Queijo", Effect::Bold))
            .content(
                LinearLayout::vertical()
                    .child(LinearLayout::vertical().with_name("users"))
                    .child(TextView::new("=".repeat(60)).h_align(HAlign::Center))
                    .child(
                        ScrollView::new(LinearLayout::vertical().with_name("messages"))
                            .scroll_strategy(ScrollStrategy::StickToBottom)
                            .resized(SizeConstraint::Full, SizeConstraint::Full),
                    )
                    .child(
                        EditView::new()
                            .on_submit(move |s, msg| {
                                s.call_on_name("message", |ev: &mut EditView| ev.set_content(""));
                                display_new_message(s, "me", msg);
                                if send_message.blocking_send(msg.into()).is_err() {
                                    s.quit();
                                }
                            })
                            .with_name("message"),
                    ),
            )
            .h_align(HAlign::Center)
            .button("Quit", |s| s.quit()),
    );

    let mut runner = cursive.runner(Backend::init().unwrap());
    runner.refresh();
    loop {
        runner.step();
        if !runner.is_running() {
            break;
        }

        loop {
            let m = match messages.try_recv() {
                Ok(m) => m,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break runner.quit(),
            };
            tracing::debug!(?m);
            match m {
                UiEvent::UserConnected { at, username } => {
                    let at_str = at.to_string();
                    let username = username.as_deref().unwrap_or(&at_str);
                    runner.call_on_name("users", |users: &mut LinearLayout| {
                        users.add_child(
                            TextView::new(format!("{username}@{at}")).with_name(&at_str),
                        );
                    });
                }
                UiEvent::UpdateUserName { at, new_username } => {
                    let at_str = at.to_string();
                    runner.call_on_name("users", |users: &mut LinearLayout| {
                        if let Some(u) = users.find_child_from_name(&at_str) {
                            if let Some(tview) = users
                                .get_child_mut(u)
                                .unwrap()
                                .downcast_mut::<NamedView<TextView>>()
                            {
                                *tview = TextView::new(format!("{new_username}@{at}"))
                                    .with_name(&at_str);
                            }
                        }
                    });
                }
                UiEvent::UserDisconnected { at } => {
                    let at_str = at.to_string();
                    runner.call_on_name("users", |users: &mut LinearLayout| {
                        if let Some(u) = users.find_child_from_name(&at_str) {
                            users.remove_child(u);
                        }
                    });
                }
                UiEvent::NewMessage {
                    at,
                    username,
                    message,
                } => {
                    let username = username.unwrap_or_else(|| at.to_string().into());
                    display_new_message(&mut runner, &username, &message)
                }
            }
            runner.refresh();
        }
    }
}
