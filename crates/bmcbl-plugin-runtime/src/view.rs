//! Limits on the generated view contract before crossing into GPUI.

use anyhow::{Result, bail, ensure};

use crate::{ViewNode, ViewTree};

const MAX_DEPTH: usize = 24;
const MAX_NODES: usize = 512;
const MAX_TEXT_BYTES: usize = 256 * 1024;
const MAX_STRING_BYTES: usize = 8192;
const MAX_INLINE_IMAGE_BYTES: usize = 128 * 1024;
const MAX_VIEW_INLINE_IMAGE_BYTES: usize = 512 * 1024;

impl ViewTree {
    /// Validates the root and its descendants, including the cost of repeated references.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.nodes.len() <= MAX_NODES,
            "plugin view has too many nodes"
        );
        let mut remaining = MAX_NODES;
        let mut text_remaining = MAX_TEXT_BYTES;
        let mut image_remaining = MAX_VIEW_INLINE_IMAGE_BYTES;
        validate(self, self.root, 1, &mut remaining, &mut text_remaining, &mut image_remaining)
    }

    /// Size of the retained WIT view and its immediate dynamic fields.
    #[must_use]
    pub fn estimated_retained_bytes(&self) -> usize {
        let mut bytes = size_of::<Self>() + self.nodes.capacity() * size_of::<ViewNode>();
        for node in &self.nodes {
            bytes += child_indices(node).len() * size_of::<u32>();
            let _ = strings(node, |value, _| {
                bytes += value.len();
                Ok(())
            });
        }
        bytes
    }
}

fn validate(
    tree: &ViewTree,
    index: u32,
    depth: usize,
    remaining: &mut usize,
    text: &mut usize,
    images: &mut usize,
) -> Result<()> {
    ensure!(
        depth <= MAX_DEPTH && *remaining != 0,
        "plugin view exceeds its tree budget"
    );
    *remaining -= 1;
    let Some(node) = tree.nodes.get(index as usize) else {
        bail!("plugin view references missing node {index}");
    };
    strings(node, |value, max_len| {
        let (remaining, kind) = if max_len == MAX_INLINE_IMAGE_BYTES {
            (&mut *images, "inline image")
        } else {
            (&mut *text, "text")
        };
        ensure!(
            value.len() <= max_len,
            "plugin view node {index}: {kind} field is {} bytes (limit {max_len})",
            value.len()
        );
        ensure!(
            value.len() <= *remaining,
            "plugin view node {index}: {kind} budget exhausted ({} bytes requested, {} remaining)",
            value.len(), *remaining
        );
        *remaining -= value.len();
        Ok(())
    })?;
    let action = match node {
        ViewNode::Button(node) => Some(node.action_id.as_str()),
        ViewNode::Input(node) => Some(node.action_id.as_str()),
        ViewNode::Checkbox(node) => Some(node.action_id.as_str()),
        ViewNode::Toggle(node) => Some(node.action_id.as_str()),
        ViewNode::Select(node) => Some(node.action_id.as_str()),
        ViewNode::Icon(node) => Some(node.name.as_str()),
        _ => None,
    };
    if let Some(action) = action {
        ensure!(
            (1..=128).contains(&action.len())
                && action.bytes().all(|b| {
                    b.is_ascii_lowercase()
                        || b.is_ascii_digit()
                        || matches!(b, b'.' | b'-' | b'_' | b':')
                }),
            "invalid plugin action or icon name"
        );
    }
    for child in child_indices(node) {
        validate(tree, *child, depth + 1, remaining, text, images)?;
    }
    Ok(())
}

fn child_indices(node: &ViewNode) -> &[u32] {
    match node {
        ViewNode::Container(node) => &node.children,
        ViewNode::ItemList(node) => &node.items,
        _ => &[],
    }
}

fn strings(node: &ViewNode, mut visit: impl FnMut(&str, usize) -> Result<()>) -> Result<()> {
    let values: &[&str] = match node {
        ViewNode::Text(n) => &[&n.text],
        ViewNode::Button(n) => &[
            &n.label,
            &n.action_id,
            n.action_value.as_deref().unwrap_or(""),
        ],
        ViewNode::Input(n) => &[
            &n.value,
            &n.placeholder,
            &n.action_id,
            n.action_value.as_deref().unwrap_or(""),
        ],
        ViewNode::Checkbox(n) => &[
            &n.label,
            &n.action_id,
            n.action_value.as_deref().unwrap_or(""),
        ],
        ViewNode::Toggle(n) => &[
            &n.label,
            &n.action_id,
            n.action_value.as_deref().unwrap_or(""),
        ],
        ViewNode::Select(n) => {
            ensure!(
                n.options.len() <= 128,
                "plugin selection has too many options"
            );
            for option in &n.options {
                visit(&option.label, MAX_STRING_BYTES)?;
                visit(&option.value, MAX_STRING_BYTES)?;
            }
            &[&n.label, &n.action_id, n.selected.as_deref().unwrap_or("")]
        }
        ViewNode::Progress(n) => &[&n.label],
        ViewNode::Link(n) => &[&n.label, &n.url, n.tooltip.as_deref().unwrap_or("")],
        ViewNode::Badge(n) => &[&n.label],
        ViewNode::Icon(n) => &[&n.name],
        ViewNode::Image(n) => {
            let inline = n.src.starts_with("data:image/png;base64,")
                || n.src.starts_with("data:image/jpeg;base64,");
            visit(&n.src, if inline { MAX_INLINE_IMAGE_BYTES } else { MAX_STRING_BYTES })?;
            &[&n.alt, &n.caption, &n.placeholder, &n.fallback]
        }
        _ => &[],
    };
    for value in values {
        visit(value, MAX_STRING_BYTES)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Align, ContainerNode, LayoutDirection, TextSizeToken, ViewStyle};

    #[test]
    fn inline_images_have_a_separate_bounded_field_budget() {
        let style = ViewStyle {
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
        let mut tree = ViewTree {
            root: 0,
            nodes: vec![ViewNode::Image(crate::ImageNode {
                src: format!("data:image/jpeg;base64,{}", "A".repeat(100 * 1024)),
                alt: "world".into(),
                caption: String::new(),
                placeholder: String::new(),
                fallback: String::new(),
                style,
                height: Some(56),
                min_height: Some(56),
                max_height: Some(56),
                aspect_ratio_x: Some(1),
                aspect_ratio_y: Some(1),
                corner_radius: Some(8),
                fit: crate::ImageFit::Cover,
            })],
        };
        assert!(tree.validate().is_ok());
        let ViewNode::Image(image) = &mut tree.nodes[0] else { unreachable!() };
        image.alt = "A".repeat(MAX_STRING_BYTES + 1);
        assert!(tree.validate().is_err(), "ordinary strings keep the 8 KiB limit");
        let ViewNode::Image(image) = &mut tree.nodes[0] else { unreachable!() };
        image.alt.clear();
        image.src = format!("data:image/jpeg;base64,{}", "A".repeat(MAX_INLINE_IMAGE_BYTES));
        assert!(tree.validate().is_err(), "image source remains bounded");

        let ViewNode::Image(image) = &mut tree.nodes[0] else { unreachable!() };
        image.src = format!("data:image/jpeg;base64,{}", "A".repeat(100 * 1024));
        let style = image.style.clone();
        tree.nodes.push(ViewNode::Container(ContainerNode {
            style,
            children: vec![0, 0, 0],
        }));
        tree.root = 1;
        assert!(tree.validate().is_ok(), "image bytes do not consume text budget");
        let ViewNode::Container(container) = &mut tree.nodes[1] else { unreachable!() };
        container.children = vec![0; 6];
        assert!(tree.validate().is_err(), "repeated images consume the aggregate image budget");
    }

    #[test]
    fn rejects_cyclic_child_indices() {
        let style = ViewStyle {
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
        let tree = ViewTree {
            root: 0,
            nodes: vec![ViewNode::Container(ContainerNode {
                style,
                children: vec![0],
            })],
        };
        assert!(tree.validate().is_err());
    }
}
