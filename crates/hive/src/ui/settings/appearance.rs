//! Appearance page: theme, fonts, UI font size and scrollbars.

use gpui_kit::component::button::{Button, ButtonGroup};
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{IndexPath, Selectable as _, Sizable as _, v_flex};
use gpui_kit::*;
use hive_core::settings::{
    Scrollbars, ThemeChoice, UI_FONT_SIZE_DEFAULT, UI_FONT_SIZE_MAX, UI_FONT_SIZE_MIN,
};

use super::{font_size_stepper, setting_row, setting_title};
use crate::app::AppState;
use crate::state::store::Store;

/// The picker entry that clears the stored family.
const DEFAULT_FONT: &str = "Default";
const FONT_PICKER_WIDTH: Pixels = px(220.);

type FontPicker = Entity<SelectState<SearchableVec<String>>>;

pub(super) struct AppearancePage {
    ui_font: FontPicker,
    code_font: FontPicker,
    _load_fonts: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl AppearancePage {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = AppState::get(cx).store.clone();
        let settings = &store.read(cx).state.settings;
        let ui_family = settings.ui_font_family().map(str::to_owned);
        let code_family = settings.code_font_family().map(str::to_owned);
        let ui_font = font_picker(ui_family.as_deref(), window, cx);
        let code_font = font_picker(code_family.as_deref(), window, cx);
        // Enumerating system fonts takes around a hundred milliseconds on macOS.
        let text_system = cx.text_system().clone();
        let fonts = cx
            .background_executor()
            .spawn(async move { text_system.all_font_names() });
        let load_fonts = cx.spawn_in(window, async move |this, cx| {
            let installed = fonts.await;
            let _ = this.update_in(cx, |this, window, cx| {
                for (picker, current) in
                    [(&this.ui_font, &ui_family), (&this.code_font, &code_family)]
                {
                    let choices = font_choices(&installed, current.as_deref());
                    picker.update(cx, |picker, cx| {
                        picker.set_items(SearchableVec::new(choices), window, cx)
                    });
                }
            });
        });
        let subscriptions = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            subscribe_font(&ui_font, &store, Store::set_ui_font_family, window, cx),
            subscribe_font(&code_font, &store, Store::set_code_font_family, window, cx),
        ];
        Self {
            ui_font,
            code_font,
            _load_fonts: load_fonts,
            _subscriptions: subscriptions,
        }
    }
}

fn font_picker(current: Option<&str>, window: &mut Window, cx: &mut App) -> FontPicker {
    let choices = font_choices(&[], current);
    let selected = Some(IndexPath::default().row(choices.len() - 1));
    cx.new(|cx| {
        SelectState::new(SearchableVec::new(choices), selected, window, cx).searchable(true)
    })
}

fn subscribe_font(
    picker: &FontPicker,
    store: &Entity<Store>,
    set: fn(&mut Store, Option<&str>, &mut Context<Store>),
    window: &mut Window,
    cx: &mut Context<AppearancePage>,
) -> Subscription {
    let store = store.clone();
    cx.subscribe_in(
        picker,
        window,
        move |_, _, event: &SelectEvent<SearchableVec<String>>, _, cx| {
            let SelectEvent::Confirm(Some(choice)) = event else {
                return;
            };
            store.update(cx, |store, cx| set(store, chosen_family(choice), cx));
        },
    )
}

/// `Default` first, then the installed families a user can pick. The saved family stays listed
/// even when it is no longer installed, so the picker still shows what is stored.
fn font_choices(installed: &[String], current: Option<&str>) -> Vec<String> {
    let mut choices = vec![DEFAULT_FONT.to_owned()];
    // Names starting with a dot are macOS private system families.
    choices.extend(
        installed
            .iter()
            .filter(|name| !name.starts_with('.'))
            .cloned(),
    );
    if let Some(current) = current.filter(|current| !choices.iter().any(|name| name == current)) {
        choices.push(current.to_owned());
    }
    choices
}

/// The family to store for a picker entry; `Default` clears it.
fn chosen_family(choice: &str) -> Option<&str> {
    (choice != DEFAULT_FONT).then_some(choice)
}

impl Render for AppearancePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = AppState::get(cx).store.clone();
        let settings = &store.read(cx).state.settings;
        let (theme, font_size, scrollbars) =
            (settings.theme, settings.ui_font_size(), settings.scrollbars);

        let themes =
            choice_group("theme", &ThemeChoice::ALL, theme, ThemeChoice::label).on_click({
                let store = store.clone();
                move |clicked: &Vec<usize>, _, cx| {
                    if let Some(theme) = clicked.first().and_then(|ix| ThemeChoice::ALL.get(*ix)) {
                        store.update(cx, |store, cx| store.set_theme(*theme, cx));
                    }
                }
            });
        let scrollbar_choices = choice_group(
            "scrollbars",
            &Scrollbars::ALL,
            scrollbars,
            Scrollbars::label,
        )
        .on_click({
            let store = store.clone();
            move |clicked: &Vec<usize>, _, cx| {
                if let Some(choice) = clicked.first().and_then(|ix| Scrollbars::ALL.get(*ix)) {
                    store.update(cx, |store, cx| store.set_scrollbars(*choice, cx));
                }
            }
        });
        let font_size = font_size_stepper(
            "ui-font-size",
            font_size,
            UI_FONT_SIZE_MIN..=UI_FONT_SIZE_MAX,
            UI_FONT_SIZE_DEFAULT,
            move |size, cx| store.update(cx, |store, cx| store.set_ui_font_size(size, cx)),
            window,
            cx,
        );

        v_flex()
            .child(setting_row(
                setting_title(
                    "Theme",
                    "Follow the system, or always use light or dark.",
                    cx,
                ),
                themes,
                cx,
            ))
            .child(setting_row(
                setting_title("UI font", "Typeface of menus, panels and dialogs.", cx),
                font_select(&self.ui_font, "UI font"),
                cx,
            ))
            .child(setting_row(
                setting_title(
                    "UI font size",
                    "Scales text and controls in every window.",
                    cx,
                ),
                font_size,
                cx,
            ))
            .child(setting_row(
                setting_title(
                    "Code font",
                    "Monospace typeface of terminals, diffs and code.",
                    cx,
                ),
                font_select(&self.code_font, "Code font"),
                cx,
            ))
            .child(setting_row(
                setting_title(
                    "Scrollbars",
                    "Follow macOS, or show while scrolling, on hover or always.",
                    cx,
                ),
                scrollbar_choices,
                cx,
            ))
    }
}

fn font_select(picker: &FontPicker, label: &'static str) -> impl IntoElement {
    Select::new(picker)
        .small()
        .w(FONT_PICKER_WIDTH)
        .accessibility_label(label)
        .search_placeholder("Search fonts")
}

/// A segmented control with one button per choice, the current one selected.
fn choice_group<T: Copy + PartialEq>(
    id: &'static str,
    choices: &[T],
    current: T,
    label: fn(T) -> &'static str,
) -> ButtonGroup {
    ButtonGroup::new(id)
        .outline()
        .small()
        .children(choices.iter().map(|choice| {
            Button::new(label(*choice))
                .cursor_pointer()
                .label(label(*choice))
                .selected(*choice == current)
        }))
}

#[cfg(test)]
mod tests {
    use super::{chosen_family, font_choices};

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn font_choices_start_with_default_and_hide_private_families() {
        let installed = names(&[".SF NS", "Inter", "Menlo"]);
        assert_eq!(
            font_choices(&installed, Some("Menlo")),
            names(&["Default", "Inter", "Menlo"])
        );
        assert_eq!(font_choices(&[], None), names(&["Default"]));
    }

    #[test]
    fn font_choices_keep_a_saved_family_that_is_not_installed() {
        let installed = names(&["Menlo"]);
        assert_eq!(
            font_choices(&installed, Some("Gone Mono")),
            names(&["Default", "Menlo", "Gone Mono"])
        );
        assert_eq!(
            font_choices(&[], Some("Inter")),
            names(&["Default", "Inter"])
        );
    }

    #[test]
    fn default_entry_clears_the_family() {
        assert_eq!(chosen_family("Default"), None);
        assert_eq!(chosen_family("Inter"), Some("Inter"));
    }
}
