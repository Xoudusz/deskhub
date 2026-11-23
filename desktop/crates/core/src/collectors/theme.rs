//! The board palette ("theme" message): published at start and whenever the theme or
//! the board-theming switch changes; the Publisher resends it after every reconnect.

use super::Ctx;
use crate::theme::theme_message;

pub async fn run(mut ctx: Ctx) {
    let mut last = theme_message(&ctx.settings.borrow_and_update());
    ctx.out.publish("theme", last.clone());
    while ctx.settings.changed().await.is_ok() {
        let next = theme_message(&ctx.settings.borrow_and_update());
        if next != last {
            ctx.out.publish("theme", next.clone());
            last = next;
        }
    }
}
