use super::*;
use gpui::{Context, Render, TestAppContext, size};

struct JoinRow {
    tree: ViewTree,
    input: Option<gpui::Entity<InputState>>,
}

impl Render for JoinRow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = crate::ui::components::page_shell::PAGE_INSET_X * 2.
            + crate::ui::components::page_shell::SPLIT_PAGE_SIDEBAR_WIDTH
            + crate::ui::components::page_shell::SPLIT_PAGE_GAP
            + px(30.); // Tools content inset and panel border.
        let row = render_validated_view_tree(&self.tree, "fixture", Some("room"), window, cx);
        let input = window.use_keyed_state::<PluginInput>(
            "plugin-input-fixture-room-room-code-1",
            cx,
            |_, _| panic!("row input should already be registered"),
        );
        self.input = Some(input.read(cx).input.clone());
        div().w(window.bounds().size.width - chrome).child(row)
    }
}

#[gpui::test]
fn plugin_action_row_gives_remaining_width_to_input(cx: &mut TestAppContext) {
    assert_action_row(cx, TextSizeToken::Small, px(32.));
}

#[gpui::test]
fn plugin_medium_input_text_is_vertically_centered(cx: &mut TestAppContext) {
    assert_action_row(cx, TextSizeToken::Body, px(38.));
}

fn assert_action_row(cx: &mut TestAppContext, text_size: TextSizeToken, height: gpui::Pixels) {
    cx.update(|cx| cx.set_global(ThemeState::default()));
    let control = ViewStyle {
        direction: LayoutDirection::Column,
        gap: 0,
        padding: 0,
        align: Align::Start,
        color: None,
        background: None,
        text_size: TextSizeToken::Body,
        emphasis: false,
        full_width: false,
        corner_radius: None,
    };
    let button = |name: &str, color| {
        ViewNode::Button(ButtonNode {
            label: name.into(),
            action_id: name.into(),
            action_value: None,
            style: ViewStyle {
                color: Some(color),
                text_size: TextSizeToken::Small,
                ..control.clone()
            },
        })
    };
    let row = ViewTree {
        root: 0,
        nodes: vec![
            ViewNode::Container(ContainerNode {
                style: ViewStyle {
                    direction: LayoutDirection::Row,
                    align: Align::Center,
                    full_width: true,
                    gap: 8,
                    ..control.clone()
                },
                children: vec![1, 2, 3],
            }),
            ViewNode::Input(InputNode {
                value: "11".into(),
                placeholder: "P/NNNN-NNNN-SSSS-SSSS".into(),
                action_id: "room-code".into(),
                action_value: None,
                style: ViewStyle {
                    full_width: true,
                    text_size,
                    ..control.clone()
                },
            }),
            button("粘贴", ThemeToken::SecondaryText),
            button("加入", ThemeToken::Accent),
        ],
    };
    let (probe, window) = cx.add_window_view(move |_, _| JoinRow {
        tree: row,
        input: None,
    });
    let mut previous_width = px(0.);
    for width in [760., 972., 1440.] {
        window.simulate_resize(size(px(width), px(560.)));
        let input = window.debug_bounds("plugin-row-cell-1").unwrap();
        let paste = window.debug_bounds("plugin-row-cell-2").unwrap();
        let join = window.debug_bounds("plugin-row-cell-3").unwrap();
        assert!(input.size.width >= px(170.));
        assert!(input.size.width > previous_width);
        assert_eq!(input.right() + px(8.), paste.origin.x);
        assert_eq!(paste.right() + px(8.), join.origin.x);
        assert!((input.center().y - paste.center().y).abs() <= px(1.));
        assert!((paste.center().y - join.center().y).abs() <= px(1.));
        assert_eq!(join.right(), px(width - 370.));
        assert_eq!(input.size.height, height);
        let text = window.update(|_, cx| {
            probe
                .read(cx)
                .input
                .as_ref()
                .unwrap()
                .read(cx)
                .text_bounds()
                .unwrap()
        });
        assert!(text.top() > input.top() && text.bottom() < input.bottom());
        assert!((text.center().y - input.center().y).abs() <= px(1.));
        previous_width = input.size.width;
    }
}

struct ProgressFixture(ViewTree);

impl Render for ProgressFixture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tree = render_validated_view_tree(&self.0, "fixture", Some("progress"), window, cx);
        div().w(window.bounds().size.width - px(370.)).child(tree)
    }
}

#[gpui::test]
fn plugin_progress_uses_available_width_and_real_fraction(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(ThemeState::default()));
    let tree = ViewTree {
        root: 0,
        nodes: vec![ViewNode::Progress(ProgressNode {
            label: "转换区块".into(),
            value: 37,
            total: Some(100),
            style: ViewStyle {
                direction: LayoutDirection::Column,
                gap: 0,
                padding: 0,
                align: Align::Stretch,
                color: None,
                background: None,
                text_size: TextSizeToken::Body,
                emphasis: false,
                full_width: true,
                corner_radius: None,
            },
        })],
    };
    let (_probe, window) = cx.add_window_view(move |_, _| ProgressFixture(tree));
    for width in [760., 972., 1440.] {
        window.simulate_resize(size(px(width), px(560.)));
        let track = window.debug_bounds("plugin-progress-track-0").unwrap();
        let fill = window.debug_bounds("plugin-progress-fill-0").unwrap();
        assert!((track.size.width - px(width - 370.)).abs() <= px(1.));
        assert!((fill.size.width - track.size.width * 0.37).abs() <= px(1.));
    }
}
