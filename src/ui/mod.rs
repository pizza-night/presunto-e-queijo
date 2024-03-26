mod backend;
mod views;

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

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

use self::views::MessageView;

#[derive(Debug)]
pub enum Event {
    UserConnected {
        at: SocketAddr,
        username: Option<Str>,
    },
    UserDisconnected {
        at: SocketAddr,
        username: Option<Str>,
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

#[derive(Debug)]
pub enum Request {
    SendMessage { body: Str },
    ChangeName { name: Str },
}

mod named_views {
    pub const TEXT_BOX: &str = "text-box";
    pub const MESSAGES: &str = "messages";
    pub const USERS: &str = "users";
}

fn display_new_message(c: &mut Cursive, addr: SocketAddr, username: Str, message: Str) {
    c.call_on_name(named_views::MESSAGES, |messages: &mut LinearLayout| {
        messages.add_child(MessageView::new(addr, username, message));
    });
}

fn display_system_message(c: &mut Cursive, message: Str) {
    c.call_on_name(named_views::MESSAGES, |messages: &mut LinearLayout| {
        messages.add_child(MessageView::new(SYSTEM_ADDR, "[system]".into(), message));
    });
}

fn change_name_dialog(send_message: Sender<Request>) -> Dialog {
    Dialog::new()
        .title("change name")
        .content(
            LinearLayout::vertical().child(EditView::new().on_submit(move |s, msg| {
                if send_message
                    .blocking_send(Request::ChangeName { name: msg.into() })
                    .is_err()
                {
                    s.quit()
                } else {
                    s.pop_layer();
                }
            })),
        )
        .h_align(HAlign::Center)
}

const SYSTEM_ADDR: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));

pub fn ui(mut messages: Receiver<Event>, send_message: Sender<Request>) {
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
                    .child(LinearLayout::vertical().with_name(named_views::USERS))
                    .child(TextView::new("=".repeat(60)).h_align(HAlign::Center))
                    .child(
                        ScrollView::new(LinearLayout::vertical().with_name(named_views::MESSAGES))
                            .scroll_strategy(ScrollStrategy::StickToBottom)
                            .resized(SizeConstraint::Full, SizeConstraint::Full),
                    )
                    .child(
                        EditView::new()
                            .on_submit({
                                let send_message = send_message.clone();
                                move |s, msg| {
                                    s.call_on_name(named_views::TEXT_BOX, |ev: &mut EditView| {
                                        ev.set_content("")
                                    });
                                    display_new_message(
                                        s,
                                        (Ipv4Addr::LOCALHOST, 2504).into(),
                                        "me".into(),
                                        msg.into(),
                                    );
                                    if send_message
                                        .blocking_send(Request::SendMessage { body: msg.into() })
                                        .is_err()
                                    {
                                        s.quit();
                                    }
                                }
                            })
                            .with_name(named_views::TEXT_BOX),
                    ),
            )
            .h_align(HAlign::Center)
            .button("Quit", |s| s.quit())
            .button("Change name", move |s| {
                s.add_layer(change_name_dialog(send_message.clone()));
            }),
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
                Event::UserConnected { at, username } => {
                    let at_str = at.to_string();
                    let username = username.as_deref().unwrap_or(&at_str);
                    runner.call_on_name(named_views::USERS, |users: &mut LinearLayout| {
                        users.add_child(
                            TextView::new(format!("{username}@{at}")).with_name(&at_str),
                        );
                    });
                    display_system_message(
                        &mut runner,
                        format!("user {username} connected").into(),
                    );
                }
                Event::UpdateUserName { at, new_username } => {
                    let at_str = at.to_string();
                    runner.call_on_name(named_views::USERS, |users: &mut LinearLayout| {
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
                    runner.call_on_name(named_views::MESSAGES, |messages: &mut LinearLayout| {
                        for i in 0..messages.len() {
                            let msg = messages
                                .get_child_mut(i)
                                .unwrap()
                                .downcast_mut::<MessageView>()
                                .unwrap();

                            if msg.addr() == &at {
                                msg.set_author(new_username.clone());
                            }
                        }
                    });
                }
                Event::UserDisconnected { at, username } => {
                    let at_str = at.to_string();
                    runner.call_on_name(named_views::USERS, |users: &mut LinearLayout| {
                        if let Some(u) = users.find_child_from_name(&at_str) {
                            users.remove_child(u);
                        }
                    });
                    let username = username.as_deref().unwrap_or(&at_str);
                    display_system_message(
                        &mut runner,
                        format!("user {username} disconnected").into(),
                    );
                }
                Event::NewMessage {
                    at,
                    username,
                    message,
                } => {
                    let username = username.unwrap_or_else(|| at.to_string().into());
                    display_new_message(&mut runner, at, username, message)
                }
            }
            runner.refresh();
        }
    }
}
