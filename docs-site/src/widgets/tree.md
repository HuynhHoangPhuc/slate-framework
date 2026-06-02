# Tree

**`Tree`** renders a caller-owned forest of `TreeNode`s as an indented column of
expand/collapse rows. The currently-visible nodes form a 1-D roving-focus ring
(same pattern as [List](list-and-virtual-list.md)).

## Building the node forest

```rust
use slate_framework::TreeNode;

fn process_tree() -> Vec<TreeNode> {
    vec![
        TreeNode::new(0, "System").children([
            TreeNode::new(1, "kernel_task"),
            TreeNode::new(2, "WindowServer"),
        ]),
        TreeNode::new(10, "Applications").children([
            TreeNode::new(11, "slate-framework"),
            TreeNode::new(12, "Terminal"),
        ]),
    ]
}
```

> Source: [`examples/dashboard/src/data.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/dashboard/src/data.rs).

## Usage

```rust
use slate_framework::Tree;

Tree::new(
    data::process_tree(),
    self.tree_expanded.clone(),  // Signal<HashSet<u64>> of expanded node ids
    self.tree_selected.clone(),  // Signal<Option<u64>> of the selected node id
)
.label("Process hierarchy")
.on_activate(|id, _| log::info!("tree node {id} activated"))
```

> Source: [`examples/dashboard/src/panels.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/dashboard/src/panels.rs).

## Key props

| Builder | Effect |
|---|---|
| `Tree::new(nodes, expanded, selected)` | Forest + `Signal<HashSet<u64>>` (expansion) + `Signal<Option<u64>>` (selection). |
| `TreeNode::new(id, label)` | A node; ids must be stable + unique (used as keys). |
| `.children([..])` | Attach child nodes. |
| `.label(name)` | Accessible name for the container. |
| `.on_activate(Fn(u64, &mut EventCtx))` | Fired on Enter / click, after selection is written. |

Both state signals are caller-owned (Strategy A) — create them outside `render`
so they survive the rebuild each toggle triggers.

## Accessibility

- **Role:** `Tree` → `TreeItem` subtree.
- **Expansion:** expandable nodes carry `is_expanded` and expose **Expand /
  Collapse** actions.
- **Keyboard:** the container is the tab stop; **arrow keys** rove focus over the
  visible nodes (real focus moves); **Enter** activates. After an expand/collapse
  the active row is re-clamped so focus is never stranded past the shortened list.
