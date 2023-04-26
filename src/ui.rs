use cursive::{
    align::HAlign,
    backends::curses::n::Backend,
    view::{Nameable, Resizable, ScrollStrategy, SizeConstraint},
    views::{Dialog, EditView, LinearLayout, ScrollView, TextView},
    Cursive,
};
use tokio::sync::mpsc::{error::TryRecvError, Receiver, Sender};

use crate::message::ArcStr;

pub enum TuiMessage {
    UserConnected { username: ArcStr },
    UserDisconnected { username: ArcStr },
    NewMessage { username: ArcStr, message: ArcStr },
}

pub fn ui(mut messages: Receiver<TuiMessage>, send_message: Sender<ArcStr>) {
    let mut cursive = Cursive::default();
    cursive.add_layer(
        Dialog::new()
            .title("Presunto e Queijo")
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
                        users.add_child(TextView::new(&username[..]).with_name(&username[..]));
                    });
                }
                TuiMessage::UserDisconnected { username } => {
                    runner.call_on_name("users", |users: &mut LinearLayout| {
                        if let Some(u) = users.find_child_from_name(&username[..]) {
                            users.remove_child(u);
                        }
                    });
                }
                TuiMessage::NewMessage { username, message } => {
                    runner.call_on_name("messages", |messages: &mut LinearLayout| {
                        messages.add_child(TextView::new(format!("{}: {}", username, message)));
                    });
                }
            }
            runner.refresh();
        }
    }
}
