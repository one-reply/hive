use super::*;
use crate::state::documents::PreviewState;

const CHECKER_SIZE: f32 = 16.;

pub(super) fn render(state: &PreviewState, cx: &App) -> AnyElement {
    match state {
        PreviewState::Loading => div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .child(gpui_kit::component::spinner::Spinner::new())
            .into_any_element(),
        PreviewState::Failed(error) => div()
            .flex_1()
            .p_4()
            .text_color(cx.theme().danger)
            .child(error.clone())
            .into_any_element(),
        PreviewState::Ready {
            image,
            omitted_resources,
        } => {
            let background = cx.theme().background;
            let checker = cx.theme().muted;
            v_flex().flex_1().min_h_0().min_w_0()
                .when(*omitted_resources, |view| view.child(div().p_2().text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Some SVG resources were omitted: external references, nested SVGs, or image limits.")))
                .child(div().relative().flex_1().min_h_0().min_w_0().overflow_hidden()
                    .child(canvas(|_, _, _| (), move |bounds, _, window, _| {
                        window.paint_quad(fill(bounds, background));
                        for (x, y) in checker_cells(bounds.size.width.into(), bounds.size.height.into()) {
                            let origin = bounds.origin + point(px(x), px(y));
                            let tile = Bounds::new(origin, size(px(CHECKER_SIZE), px(CHECKER_SIZE)))
                                .intersect(&bounds);
                            window.paint_quad(fill(tile, checker));
                        }
                    }).absolute().size_full())
                    .child(img(image.clone()).id("file-image").role(Role::Image)
                        .aria_label("Image preview").object_fit(ObjectFit::Contain).absolute().size_full()))
                .into_any_element()
        }
    }
}

fn checker_cells(width: f32, height: f32) -> impl Iterator<Item = (f32, f32)> {
    let columns = (width / CHECKER_SIZE).ceil() as usize;
    let rows = (height / CHECKER_SIZE).ceil() as usize;
    (0..rows).flat_map(move |row| {
        (0..columns)
            .filter(move |column| (column + row) % 2 == 0)
            .map(move |column| (column as f32 * CHECKER_SIZE, row as f32 * CHECKER_SIZE))
    })
}

#[cfg(test)]
mod tests {
    use super::checker_cells;

    #[test]
    fn checkerboard_alternates_and_covers_partial_cells() {
        assert_eq!(
            checker_cells(33., 17.).collect::<Vec<_>>(),
            vec![(0., 0.), (32., 0.), (16., 16.)]
        );
        assert_eq!(checker_cells(0., 100.).count(), 0);
        assert_eq!(checker_cells(100., 0.).count(), 0);
    }
}
