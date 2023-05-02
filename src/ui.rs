use cursive::{
    align::HAlign,
    backends::curses::n::Backend,
    theme::Effect,
    utils::span::SpannedString,
    view::{Nameable, Resizable, ScrollStrategy, SizeConstraint},
    views::{Dialog, EditView, LinearLayout, ScrollView, TextView},
    Cursive,
};
use tokio::sync::mpsc::{error::TryRecvError, Receiver, Sender};

use crate::Str;

pub enum TuiMessage {
    UserConnected { username: Str },
    UserDisconnected { username: Str },
    NewMessage { username: Str, message: Str },
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

pub fn ui(mut messages: Receiver<TuiMessage>, send_message: Sender<Str>) {
    let mut cursive = Cursive::default();
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
                    .child(LinearLayout::horizontal().with_name("users"))
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
            match m {
                TuiMessage::UserConnected { username } => {
                    runner.call_on_name("users", |users: &mut LinearLayout| {
                        users.add_child(TextView::new(&*username).with_name(&*username));
                    });
                }
                TuiMessage::UserDisconnected { username } => {
                    runner.call_on_name("users", |users: &mut LinearLayout| {
                        if let Some(u) = users.find_child_from_name(&username) {
                            users.remove_child(u);
                        }
                    });
                }
                TuiMessage::NewMessage { username, message } => {
                    display_new_message(&mut runner, &username, &message)
                }
            }
            runner.refresh();
        }
    }
}
