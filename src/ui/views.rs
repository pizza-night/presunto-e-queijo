use std::net::SocketAddr;

use cursive::{
    theme::{Effect, Style},
    utils::span::{IndexedSpan, SpannedStr},
    views::TextView,
    Vec2, View,
};

use crate::Str;

pub struct MessageView {
    addr: SocketAddr,
    author: Str,
    author_len: usize,
    needs_relayout: bool,
    message: TextView,
}

impl MessageView {
    pub fn new(addr: SocketAddr, author: Str, message: Str) -> Self {
        Self {
            addr,
            author_len: author.chars().count(),
            author,
            needs_relayout: true,
            message: TextView::new(message),
        }
    }

    pub fn set_author(&mut self, author: Str) {
        self.author = author;
        self.author_len = self.author.chars().count();
        self.needs_relayout = true;
    }

    pub fn addr(&self) -> &SocketAddr {
        &self.addr
    }
}

impl View for MessageView {
    fn draw(&self, printer: &cursive::Printer) {
        printer.print_styled(
            (0, 0),
            SpannedStr::new(
                &self.author,
                &[IndexedSpan::simple_borrowed(
                    &self.author,
                    Style::from(Effect::Bold),
                )],
            ),
        );
        printer.print((self.author_len, 0), ": ");
        self.message.draw(&printer.offset((self.author_len + 2, 0)));
    }

    fn needs_relayout(&self) -> bool {
        self.needs_relayout || self.message.needs_relayout()
    }

    fn required_size(&mut self, constraint: Vec2) -> Vec2 {
        self.message.required_size(constraint)
    }

    fn layout(&mut self, size: Vec2) {
        self.message
            .layout((size.x + self.author_len + 2, size.y).into())
    }
}
