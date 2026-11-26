use super::*;
use crate::{
    Font, FontFeatures, FontStyle, FontWeight, Hsla, TestAppContext, TestDispatcher, TextRun, font,
    px,
};
#[cfg(target_os = "macos")]
use crate::{WindowTextSystem, WrapBoundary};
use rand::prelude::*;

fn build_wrapper() -> LineWrapper {
    let dispatcher = TestDispatcher::new(StdRng::seed_from_u64(0));
    let cx = TestAppContext::build(dispatcher, None);
    let id = cx.text_system().resolve_font(&font(".ZedMono"));
    LineWrapper::new(id, px(16.), cx.text_system().platform_text_system.clone())
}

fn generate_test_runs(input_run_len: &[usize]) -> Vec<TextRun> {
    input_run_len
        .iter()
        .map(|run_len| TextRun {
            len: *run_len,
            font: Font {
                family: "Dummy".into(),
                features: FontFeatures::default(),
                fallbacks: None,
                weight: FontWeight::default(),
                style: FontStyle::Normal,
            },
            color: Hsla::default(),
            background_color: None,
            background_corner_radius: None,
            background_padding: None,
            underline: None,
            strikethrough: None,
        })
        .collect()
}

#[test]
fn test_wrap_line() {
    let mut wrapper = build_wrapper();

    assert_eq!(
        wrapper
            .wrap_line(&[LineFragment::text("aa bbb cccc ddddd eeee")], px(72.))
            .collect::<Vec<_>>(),
        &[
            Boundary::new(7, 0),
            Boundary::new(12, 0),
            Boundary::new(18, 0)
        ],
    );
    assert_eq!(
        wrapper
            .wrap_line(&[LineFragment::text("aaa aaaaaaaaaaaaaaaaaa")], px(72.0))
            .collect::<Vec<_>>(),
        &[
            Boundary::new(4, 0),
            Boundary::new(11, 0),
            Boundary::new(18, 0)
        ],
    );
    assert_eq!(
        wrapper
            .wrap_line(&[LineFragment::text("     aaaaaaa")], px(72.))
            .collect::<Vec<_>>(),
        &[
            Boundary::new(7, 5),
            Boundary::new(9, 5),
            Boundary::new(11, 5),
        ]
    );
    assert_eq!(
        wrapper
            .wrap_line(
                &[LineFragment::text("                            ")],
                px(72.)
            )
            .collect::<Vec<_>>(),
        &[
            Boundary::new(7, 0),
            Boundary::new(14, 0),
            Boundary::new(21, 0)
        ]
    );
    assert_eq!(
        wrapper
            .wrap_line(&[LineFragment::text("          aaaaaaaaaaaaaa")], px(72.))
            .collect::<Vec<_>>(),
        &[
            Boundary::new(7, 0),
            Boundary::new(14, 3),
            Boundary::new(18, 3),
            Boundary::new(22, 3),
        ]
    );

    assert_eq!(
        wrapper
            .wrap_line(
                &[
                    LineFragment::text("aa bbb "),
                    LineFragment::text("cccc ddddd eeee")
                ],
                px(72.)
            )
            .collect::<Vec<_>>(),
        &[
            Boundary::new(7, 0),
            Boundary::new(12, 0),
            Boundary::new(18, 0)
        ],
    );

    assert_eq!(
        wrapper
            .wrap_line(
                &[
                    LineFragment::text("aa "),
                    LineFragment::element(px(20.), 1),
                    LineFragment::text(" bbb "),
                    LineFragment::element(px(30.), 1),
                    LineFragment::text(" cccc")
                ],
                px(72.)
            )
            .collect::<Vec<_>>(),
        &[
            Boundary::new(5, 0),
            Boundary::new(9, 0),
            Boundary::new(11, 0)
        ],
    );

    assert_eq!(
        wrapper
            .wrap_line(
                &[
                    LineFragment::element(px(50.), 1),
                    LineFragment::text(" aaaa bbbb cccc dddd")
                ],
                px(72.)
            )
            .collect::<Vec<_>>(),
        &[
            Boundary::new(2, 0),
            Boundary::new(7, 0),
            Boundary::new(12, 0),
            Boundary::new(17, 0)
        ],
    );

    assert_eq!(
        wrapper
            .wrap_line(
                &[
                    LineFragment::text("short text "),
                    LineFragment::element(px(100.), 1),
                    LineFragment::text(" more text")
                ],
                px(72.)
            )
            .collect::<Vec<_>>(),
        &[
            Boundary::new(6, 0),
            Boundary::new(11, 0),
            Boundary::new(12, 0),
            Boundary::new(18, 0)
        ],
    );
}

#[test]
fn test_truncate_line() {
    let mut wrapper = build_wrapper();

    fn perform_test(
        wrapper: &mut LineWrapper,
        text: &'static str,
        result: &'static str,
        ellipsis: &str,
    ) {
        let dummy_run_lens = vec![text.len()];
        let mut dummy_runs = generate_test_runs(&dummy_run_lens);
        assert_eq!(
            wrapper.truncate_line(text.into(), px(220.), ellipsis, &mut dummy_runs),
            result
        );
        assert_eq!(dummy_runs.first().unwrap().len, result.len());
    }

    perform_test(
        &mut wrapper,
        "aa bbb cccc ddddd eeee ffff gggg",
        "aa bbb cccc ddddd eeee",
        "",
    );
    perform_test(
        &mut wrapper,
        "aa bbb cccc ddddd eeee ffff gggg",
        "aa bbb cccc ddddd eee…",
        "…",
    );
    perform_test(
        &mut wrapper,
        "aa bbb cccc ddddd eeee ffff gggg",
        "aa bbb cccc dddd......",
        "......",
    );
}

#[test]
fn test_truncate_multiple_runs() {
    let mut wrapper = build_wrapper();

    fn perform_test(
        wrapper: &mut LineWrapper,
        text: &'static str,
        result: &str,
        run_lens: &[usize],
        result_run_len: &[usize],
        line_width: crate::Pixels,
    ) {
        let mut dummy_runs = generate_test_runs(run_lens);
        assert_eq!(
            wrapper.truncate_line(text.into(), line_width, "…", &mut dummy_runs),
            result
        );
        for (run, result_len) in dummy_runs.iter().zip(result_run_len) {
            assert_eq!(run.len, *result_len);
        }
    }
    perform_test(&mut wrapper, "abcdefghijkl", "abcd…", &[12], &[7], px(50.));
    perform_test(
        &mut wrapper,
        "abcdefghijkl",
        "abcdef…",
        &[4, 4, 4],
        &[4, 5],
        px(70.),
    );
    perform_test(
        &mut wrapper,
        "abcdefghijkl",
        "abcdefgh…",
        &[4, 4, 4],
        &[4, 4, 3],
        px(90.),
    );
}

#[test]
fn test_update_run_after_truncation() {
    fn perform_test(result: &str, run_lens: &[usize], result_run_lens: &[usize]) {
        let mut dummy_runs = generate_test_runs(run_lens);
        super::truncate::update_runs_after_truncation(result, "…", &mut dummy_runs);
        for (run, result_len) in dummy_runs.iter().zip(result_run_lens) {
            assert_eq!(run.len, *result_len);
        }
    }
    perform_test("abcd…", &[12], &[7]);
    perform_test("abcdef…", &[4, 4, 4], &[4, 5]);
    perform_test("abcdefgh…", &[4, 4, 4], &[4, 4, 3]);
}

#[test]
fn unicode_break_opportunities_preserve_glue_and_punctuation() {
    let allows_break = |text: &str, index| LineWrapper::break_indices(text).any(|i| i == index);

    assert!(!allows_break("a\u{00a0}b", "a\u{00a0}".len()));
    assert!(!allows_break("a\u{2060}b", "a\u{2060}".len()));
    assert!(!allows_break("（中文）", "（".len()));
    assert!(!allows_break("您好，世界", "您好".len()));
    assert!(allows_break("您好，世界", "您好，".len()));
    assert!(allows_break("a b", "a ".len()));
}

#[test]
fn forced_wrap_preserves_combining_graphemes_across_fragments() {
    let mut wrapper = build_wrapper();
    let fragments = [LineFragment::text("a"), LineFragment::text("\u{0301}b")];
    let boundaries: Vec<_> = wrapper.wrap_line(&fragments, px(1.)).collect();

    assert_eq!(boundaries, [Boundary::new("a\u{0301}".len(), 0)]);
}

#[cfg(target_os = "macos")]
use crate as gpui;

#[cfg(target_os = "macos")]
#[crate::test]
fn test_wrap_shaped_line(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let text_system = WindowTextSystem::new(cx.text_system().clone());

        let normal = TextRun {
            len: 0,
            font: font("Helvetica"),
            color: Default::default(),
            underline: Default::default(),
            strikethrough: None,
            background_color: None,
            background_corner_radius: None,
            background_padding: None,
        };
        let bold = TextRun {
            len: 0,
            font: font("Helvetica").bold(),
            color: Default::default(),
            underline: Default::default(),
            strikethrough: None,
            background_color: None,
            background_corner_radius: None,
            background_padding: None,
        };

        let text = "aa bbb cccc ddddd eeee".into();
        let lines = text_system
            .shape_text(
                text,
                px(16.),
                &[
                    normal.with_len(4),
                    bold.with_len(5),
                    normal.with_len(6),
                    bold.with_len(1),
                    normal.with_len(7),
                ],
                Some(px(72.)),
                None,
            )
            .unwrap();

        assert_eq!(
            lines[0].layout.wrap_boundaries(),
            &[
                WrapBoundary {
                    run_ix: 0,
                    glyph_ix: 7
                },
                WrapBoundary {
                    run_ix: 0,
                    glyph_ix: 12
                },
                WrapBoundary {
                    run_ix: 0,
                    glyph_ix: 18
                },
            ]
        )
    });
}

#[test]
fn cjk_break_constraints_match_gpui_fast() {
    let mut wrapper = build_wrapper();
    let text = "今天的天气非常好，我们一起去公园散步吧。";
    let fits = "今天的天气非常好";
    let width = fits.chars().map(|c| wrapper.width_for_char(c))
        .fold(px(0.), |sum, width| sum + width)
        + wrapper.width_for_char('，') / 2.;
    let boundaries = wrapper.wrap_line(&[LineFragment::text(text)], width)
        .collect::<Vec<_>>();
    assert_eq!(boundaries[0].ix, fits.len() - '好'.len_utf8());
}
