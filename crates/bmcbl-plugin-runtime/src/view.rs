//! Limits on the generated view contract before crossing into GPUI.

use anyhow::{Result, bail, ensure};

use crate::{ViewNode, ViewTree};

// These are ingress safety guards, not UI quotas. Normal retained views
// are managed by the application's shared cache, not by per-field limits.
// The recursive GPUI projection still needs a stack-depth guard.
const MAX_DEPTH: usize = 128;
const MAX_HOST_VIEW_BYTES: usize = crate::sandbox::DEFAULT_HARD_LIMIT_BYTES;

impl ViewTree {
    /// Validate the contract and bound its expanded host projection cost.
    /// Text, images, select options and node counts have no separate quotas.
    pub fn validate(&self) -> Result<()> {
        let mut remaining = MAX_HOST_VIEW_BYTES;
        let retained = self.estimated_retained_bytes();
        ensure!(retained <= remaining,
            "plugin view exceeds host ingress safety ceiling: {retained} bytes (limit {remaining})");
        // Validate all retained fields once; account repeated references during
        // traversal so a small DAG cannot amplify into an unbounded UI tree.
        let costs: Vec<_> = self.nodes.iter().map(node_bytes).collect();
        validate(self, self.root, 1, &costs, &mut remaining)
    }

    /// Estimated owned WIT allocations (not decoded image or GPU memory).
    #[must_use]
    pub fn estimated_retained_bytes(&self) -> usize {
        self.nodes.iter().fold(
            size_of::<Self>() + self.nodes.capacity().saturating_mul(size_of::<ViewNode>()),
            |bytes, node| bytes.saturating_add(node_bytes(node).saturating_sub(size_of::<ViewNode>())),
        )
    }
}

fn node_bytes(node: &ViewNode) -> usize {
    let mut bytes = size_of::<ViewNode>()
        .saturating_add(child_indices(node).len().saturating_mul(size_of::<u32>()));
    if let ViewNode::Select(node) = node {
        // Each option also owns two strings.
        bytes = bytes.saturating_add(node.options.capacity().saturating_mul(size_of::<crate::SelectOption>()));
    }
    strings(node, |value| { bytes = bytes.saturating_add(value.len()); });
    bytes
}

fn validate(
    tree: &ViewTree,
    index: u32,
    depth: usize,
    costs: &[usize],
    remaining: &mut usize,
) -> Result<()> {
    ensure!(depth <= MAX_DEPTH, "plugin view nesting exceeds host stack safety limit");
    let Some(node) = tree.nodes.get(index as usize) else {
        bail!("plugin view references missing node {index}");
    };
    let cost = costs[index as usize];
    ensure!(cost <= *remaining, "plugin view expanded host allocation exceeds safety ceiling at node {index}");
    *remaining -= cost;
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
        validate(tree, *child, depth + 1, costs, remaining)?;
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

fn strings(node: &ViewNode, mut visit: impl FnMut(&str)) {
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
            for option in &n.options {
                visit(&option.label);
                visit(&option.value);
            }
            &[&n.label, &n.action_id, n.selected.as_deref().unwrap_or("")]
        }
        ViewNode::Progress(n) => &[&n.label],
        ViewNode::Link(n) => &[&n.label, &n.url, n.tooltip.as_deref().unwrap_or("")],
        ViewNode::Badge(n) => &[&n.label],
        ViewNode::Icon(n) => &[&n.name],
        ViewNode::Image(n) => {
            visit(&n.src);
            &[&n.alt, &n.caption, &n.placeholder, &n.fallback]
        }
        _ => &[],
    };
    for value in values {
        visit(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Align, ContainerNode, LayoutDirection, TextSizeToken, ViewStyle};

    #[test]
    fn large_text_and_images_share_host_ingress_safety_ceiling() {
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
        image.alt = "A".repeat(256 * 1024);
        assert!(tree.validate().is_ok(), "ordinary text is not capped at 8 KiB");
        let ViewNode::Image(image) = &mut tree.nodes[0] else { unreachable!() };
        image.alt.clear();
        image.src = format!("data:image/jpeg;base64,{}", "A".repeat(1024 * 1024));
        assert!(tree.validate().is_ok(), "inline image has no independent small quota");

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
        container.children = vec![0; 12];
        assert!(tree.validate().is_ok(), "ordinary multi-image views exceed the former budget");
        let ViewNode::Container(container) = &mut tree.nodes[1] else { unreachable!() };
        container.children = vec![0; 512];
        assert!(tree.validate().is_err(), "expanded references cannot amplify host memory without bound");
    }

    #[test]
    fn accepts_large_native_style_node_lists() {
        let style = ViewStyle {
            direction: LayoutDirection::Column, gap: 0, padding: 0,
            align: Align::Start, color: None, background: None,
            text_size: TextSizeToken::Body, emphasis: false, full_width: false,
            corner_radius: None,
        };
        let mut nodes = vec![ViewNode::Container(ContainerNode {
            style: style.clone(), children: (1..=2048).collect(),
        })];
        nodes.extend((0..2048).map(|_| ViewNode::Text(crate::TextNode {
            text: "native UI row".into(), style: style.clone(),
        })));
        assert!(ViewTree { root: 0, nodes }.validate().is_ok());
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
