use super::*;

const EMOJIS: &[(&str, &str)] = &[
    ("🚀", "Rocket"),
    ("🛠️", "Tools"),
    ("📱", "Phone"),
    ("💻", "Laptop"),
    ("🌐", "Globe"),
    ("📦", "Package"),
    ("🤖", "Robot"),
    ("🎨", "Palette"),
    ("🎮", "Controller"),
    ("🧪", "Test tube"),
    ("🧩", "Puzzle"),
    ("⚡", "Lightning"),
    ("🔒", "Lock"),
    ("🌱", "Seedling"),
    ("🐝", "Bee"),
    ("🦀", "Crab"),
];

impl ProjectPage {
    pub(super) fn render_emojis(
        &self,
        snapshot: &Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut choices = h_flex().flex_wrap().gap_1();
        for &(emoji, name) in EMOJIS {
            let selected =
                matches!(snapshot.identity.badge(), Badge::Emoji(value) if value == emoji);
            let hover = Hover::new(emoji, window, cx);
            let button = badge_button(emoji, name, ICON_TILE, selected)
                .child(div().flex_none().text_size(ICON_SIZE).child(emoji))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.emoji_input
                        .update(cx, |input, cx| input.set_value(emoji, window, cx));
                    this.update_identity(cx, |identity| identity.set_emoji(emoji));
                }))
                .smooth_hover(badge_look(selected), &hover, cx);
            choices = choices.child(button);
        }
        v_flex().gap_2().child(choices).child(TextField::input(
            Input::new(&self.emoji_input),
            &self.emoji_input,
            cx,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::EMOJIS;
    use hive_core::identity::{Badge, ProjectIdentity};
    use std::collections::HashSet;

    #[test]
    fn presets_have_distinct_buttons_and_save_the_displayed_emoji() {
        let mut ids = HashSet::new();
        let mut identity = ProjectIdentity::default();
        for &(emoji, name) in EMOJIS {
            assert!(ids.insert(emoji), "duplicate emoji button: {emoji}");
            assert!(!name.trim().is_empty());
            identity.set_emoji(emoji);
            assert!(matches!(identity.badge(), Badge::Emoji(value) if value == emoji));
        }
    }
}
