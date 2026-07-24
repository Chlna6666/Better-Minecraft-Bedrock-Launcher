use super::*;
use crate::{BackgroundExecutor, Element, ElementId, InteractiveElement, RenderImage};
use image::{Delay, Frame, ImageBuffer};
use rand::SeedableRng as _;
use std::{sync::Arc, time::Duration};

#[test]
fn img_has_stable_fallback_element_id() {
    let image = img("examples/image/black-cat-typing.gif");
    assert_eq!(Element::id(&image), Element::id(&image));
}

#[test]
fn explicit_img_element_id_is_preserved() {
    let image = img("examples/image/black-cat-typing.gif").id("explicit");
    assert_eq!(
        Element::id(&image),
        Some(ElementId::Name("explicit".into()))
    );
}

#[test]
fn select_animation_frame_advances_resident_frames() {
    let frame = |color| {
        Frame::from_parts(
            ImageBuffer::from_pixel(1, 1, image::Rgba(color)),
            0,
            0,
            Delay::from_saturating_duration(Duration::from_millis(1)),
        )
    };
    let image = RenderImage::new(vec![frame([255, 0, 0, 255]), frame([0, 255, 0, 255])]);
    let executor = BackgroundExecutor::new(Arc::new(crate::TestDispatcher::new(
        rand::rngs::StdRng::seed_from_u64(1),
    )));
    let now = executor.now();
    let mut state = ImgState {
        current_image: None,
        current_frame: Some(image.frame(0).unwrap()),
        next_frame_at: Some(now - Duration::from_millis(1)),
        started_loading: None,
        target_size_asset: None,
        pending_target_drop: None,
    };

    let next_frame = select_animation_frame(
        &mut state,
        &image,
        crate::AnimatedImageConfig {
            max_fps: 240.0,
            ..crate::AnimatedImageConfig::default()
        },
        &executor,
    )
    .unwrap();

    assert_eq!(next_frame.sequence(), 1);
}

#[test]
fn select_animation_frame_catches_up_ready_resident_frames() {
    let frame = |color| {
        Frame::from_parts(
            ImageBuffer::from_pixel(1, 1, image::Rgba(color)),
            0,
            0,
            Delay::from_saturating_duration(Duration::from_millis(1)),
        )
    };
    let image = RenderImage::new(vec![
        frame([255, 0, 0, 255]),
        frame([0, 255, 0, 255]),
        frame([0, 0, 255, 255]),
        frame([255, 255, 0, 255]),
        frame([255, 0, 255, 255]),
    ]);
    let executor = BackgroundExecutor::new(Arc::new(crate::TestDispatcher::new(
        rand::rngs::StdRng::seed_from_u64(2),
    )));
    let now = executor.now();
    let mut state = ImgState {
        current_image: None,
        current_frame: Some(image.frame(0).unwrap()),
        next_frame_at: Some(now - Duration::from_millis(80)),
        started_loading: None,
        target_size_asset: None,
        pending_target_drop: None,
    };

    let next_frame = select_animation_frame(
        &mut state,
        &image,
        crate::AnimatedImageConfig {
            max_fps: 240.0,
            ..crate::AnimatedImageConfig::default()
        },
        &executor,
    )
    .unwrap();

    assert_eq!(next_frame.sequence(), 4);
}

#[test]
fn animated_img_requests_frames_when_policy_plays() {
    let image = RenderImage::new(vec![
        Frame::new(ImageBuffer::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]))),
        Frame::new(ImageBuffer::from_pixel(1, 1, image::Rgba([0, 255, 0, 255]))),
    ]);
    let policy = ImageAnimationPolicy::playing(12.0);
    let config = policy
        .apply_to(crate::AnimatedImageConfig::default())
        .clamped();

    assert!(should_request_image_animation_frame(&image, config));
}

#[test]
fn animated_img_does_not_request_frames_when_policy_pauses() {
    let image = RenderImage::new(vec![
        Frame::new(ImageBuffer::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]))),
        Frame::new(ImageBuffer::from_pixel(1, 1, image::Rgba([0, 255, 0, 255]))),
    ]);
    let policy = ImageAnimationPolicy::paused();
    let config = policy
        .apply_to(crate::AnimatedImageConfig::default())
        .clamped();

    assert!(!should_request_image_animation_frame(&image, config));
}

#[test]
fn static_img_does_not_request_animation_frames() {
    let image = RenderImage::new(vec![Frame::new(ImageBuffer::from_pixel(
        1,
        1,
        image::Rgba([255, 0, 0, 255]),
    ))]);
    let policy = ImageAnimationPolicy::playing(12.0);
    let config = policy
        .apply_to(crate::AnimatedImageConfig::default())
        .clamped();

    assert!(!should_request_image_animation_frame(&image, config));
}
