//! A demo of item grid windows (crawler plan CE2): `sandbox.grids_show` shows three windows, a
//! 6x4 bag, two equipment slots and a 6x12 chest that shows 4 rows and scrolls. Every window's drop
//! command is `sandbox.grid_put <window>`, which moves the item when it fits (the contract's
//! `fits_at`) and refuses otherwise, the way a real module decides. Checked by
//! `scripts/verify/item_grids.cmds` (and in the browser).

use bevy::prelude::*;
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandOutcome, CommandResult, UiGridItem, UiGridShape, UiGridSlot,
    UiGridView, UiGrids, UiTooltipLine, drawn_size, fits_at,
};

const BAG: &str = "demo_bag";
const GEAR: &str = "demo_gear";
const CHEST: &str = "demo_chest";

/// One demo item and where it is.
#[derive(Clone, Debug, PartialEq)]
struct DemoItem {
    id: &'static str,
    grid: &'static str,
    x: u32,
    y: u32,
    size: UVec2,
    rotated: bool,
    color: Color,
    image: Option<&'static str>,
}

/// The demo's state: which windows are shown and where the items are.
#[derive(Resource, Debug)]
struct DemoGrids {
    shown: bool,
    items: Vec<DemoItem>,
}

fn item(
    id: &'static str,
    grid: &'static str,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    color: Color,
) -> DemoItem {
    DemoItem {
        id,
        grid,
        x,
        y,
        size: UVec2::new(w, h),
        rotated: false,
        color,
        image: None,
    }
}

impl Default for DemoGrids {
    fn default() -> Self {
        let mut sword = item("sword", BAG, 0, 0, 1, 3, Color::srgb(0.45, 0.30, 0.20));
        sword.image = Some("textures/checker.png");
        Self {
            shown: false,
            items: vec![
                sword,
                item("shield", BAG, 2, 0, 2, 2, Color::srgb(0.20, 0.35, 0.55)),
                item("potion", BAG, 5, 3, 1, 1, Color::srgb(0.60, 0.15, 0.20)),
                item("helm", GEAR, 0, 0, 2, 2, Color::srgb(0.50, 0.50, 0.55)),
                item("gem", CHEST, 0, 9, 1, 1, Color::srgb(0.30, 0.65, 0.40)),
            ],
        }
    }
}

pub struct ItemGridsPlugin;

impl Plugin for ItemGridsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DemoGrids>()
            .register_command("sandbox.grids_show", "sandbox.grids_show", show_command)
            .register_command("sandbox.grids_hide", "sandbox.grids_hide", hide_command)
            .register_command(
                "sandbox.grid_put",
                "sandbox.grid_put <window> <item> <x> <y> <0|1>",
                put_command,
            )
            .add_systems(Update, show_demo_grids);
    }
}

fn window(grid: &'static str) -> UiGridView {
    let (title, columns, rows, visible_rows, shape, order) = match grid {
        BAG => ("Bag", 6, 4, 0, UiGridShape::Cells, 1),
        GEAR => (
            "Equipment",
            4,
            5,
            0,
            UiGridShape::Slots(vec![
                UiGridSlot {
                    label: "Head".to_string(),
                    x: 1,
                    y: 0,
                    size: UVec2::new(2, 2),
                },
                UiGridSlot {
                    label: "Hand".to_string(),
                    x: 1,
                    y: 2,
                    size: UVec2::new(2, 3),
                },
            ]),
            0,
        ),
        _ => ("Chest", 6, 12, 4, UiGridShape::Cells, 2),
    };
    UiGridView {
        title: title.to_string(),
        columns,
        rows,
        visible_rows,
        shape,
        items: Vec::new(),
        drop_command: format!("sandbox.grid_put {grid}"),
        order,
        footer: if grid == BAG {
            "Gold: 120".to_string()
        } else {
            String::new()
        },
    }
}

fn view_of(demo: &DemoGrids, grid: &'static str) -> UiGridView {
    let mut view = window(grid);
    view.items = demo
        .items
        .iter()
        .filter(|item| item.grid == grid)
        .map(|item| UiGridItem {
            id: item.id.to_string(),
            x: item.x,
            y: item.y,
            size: item.size,
            rotated: item.rotated,
            color: item.color,
            image: item.image.map(str::to_string),
            label: item.id.to_string(),
            tooltip: vec![
                UiTooltipLine::new(item.id, Color::srgb(1.0, 0.85, 0.3)),
                UiTooltipLine::new(
                    format!("{}x{} cells", item.size.x, item.size.y),
                    Color::WHITE,
                ),
            ],
        })
        .collect();
    view
}

/// Writes the windows whenever the demo changes (only what differs, so nothing rebuilds otherwise).
fn show_demo_grids(demo: Res<DemoGrids>, mut grids: ResMut<UiGrids>) {
    if !demo.is_changed() {
        return;
    }
    for grid in [BAG, GEAR, CHEST] {
        if demo.shown {
            let view = view_of(&demo, grid);
            if grids.view(grid) != Some(&view) {
                grids.set_view(grid, view);
            }
        } else if grids.view(grid).is_some() {
            grids.remove_view(grid);
        }
    }
}

fn show_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    world.resource_mut::<DemoGrids>().shown = true;
    Ok(CommandOutcome::Continue)
}

fn hide_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    world.resource_mut::<DemoGrids>().shown = false;
    Ok(CommandOutcome::Continue)
}

/// `sandbox.grid_put <window> <item> <x> <y> <0|1>`: the drop of every demo window.
fn put_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(5)?;
    let grid: String = args.parse(0, "window")?;
    let id: String = args.parse(1, "item")?;
    let x: u32 = args.parse(2, "x")?;
    let y: u32 = args.parse(3, "y")?;
    let rotated = args.parse::<u8>(4, "rotated")? == 1;
    let grid: &'static str = [BAG, GEAR, CHEST]
        .into_iter()
        .find(|known| *known == grid)
        .ok_or_else(|| args.failure(format!("`{grid}` is not a demo window")))?;
    let demo = world.resource::<DemoGrids>();
    let index = demo
        .items
        .iter()
        .position(|item| item.id == id)
        .ok_or_else(|| args.failure(format!("no demo item `{id}`")))?;
    let size = drawn_size(demo.items[index].size, rotated);
    if !fits_at(&view_of(demo, grid), &id, size, x, y) {
        return Err(args.failure(format!("`{id}` does not fit at ({x}, {y}) of {grid}")));
    }
    let mut demo = world.resource_mut::<DemoGrids>();
    let moved = &mut demo.items[index];
    moved.grid = grid;
    moved.x = x;
    moved.y = y;
    // A slot shows its item upright.
    moved.rotated = rotated && grid != GEAR;
    Ok(CommandOutcome::Continue)
}
