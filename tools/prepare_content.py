#!/usr/bin/env python3
"""Build the Create content mod from the player's own Create jar.

Create's assets are All Rights Reserved, so this repository ships none of
them. The player downloads Create's jar themselves; Horizon's own
`horizon-port-pack` carries its blocks, items, recipes, models and textures
into a content mod; this script then fits the carried blocks to the native
mod:

- rotating blocks (shaft, cogwheels) leave the chunk mesh: their blockstates
  point to an empty model, and hidden `create:hz_*` blocks hold the real
  models, which the native mod draws turning every frame;
- blocks get the templates and shapes Create gives them (a thin shaft, a
  cogwheel's disc, a crank on any face);
- belts are a block per part on a grindstone's template (its `face` holds
  the slope), drawn by the native mod; chutes, funnels and belt tunnels get
  the templates the native mod expects (see `fit_logistics`);
- the water wheel's OBJ model (`models/block/water_wheel/`), which the
  native mod reads at runtime, is taken from the jar when the carried
  content lacks it.

Usage (Python 3.9+):

  python3 tools/prepare_content.py --jar create-1.21.1-6.0.10.jar \\
      --resources <Horizon's prepared 26.3 resources> --out target/content

The result, `target/content/create`, is a content mod folder: run the game
with `--dev-mod target/content/create`, or copy it into `horizon-mods/`.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]

# Box helpers in sixteenths, turned to block units.
def box(x0, y0, z0, x1, y1, z1):
    return [x0 / 16, y0 / 16, z0 / 16, x1 / 16, y1 / 16, z1 / 16]


def along(axis, b):
    """A box modelled along Y, turned to lie along `axis`."""
    x0, y0, z0, x1, y1, z1 = b
    if axis == "y":
        return b
    if axis == "x":
        return [y0, x0, z0, y1, x1, z1]
    return [x0, z0, y0, x1, z1, y1]


def facing_box(facing, b):
    """A box modelled sitting on the floor (facing up), turned so it sits
    on the face opposite `facing`."""
    x0, y0, z0, x1, y1, z1 = b
    return {
        "up": [x0, y0, z0, x1, y1, z1],
        "down": [x0, 1 - y1, z0, x1, 1 - y0, z1],
        "north": [x0, z0, 1 - y1, x1, z1, 1 - y0],
        "south": [x0, z0, y0, x1, z1, y1],
        "west": [1 - y1, z0, x0, 1 - y0, z1, x1],
        "east": [y0, z0, x0, y1, z1, x1],
    }[facing]


AXES = ["x", "y", "z"]
FACINGS = ["down", "up", "north", "south", "west", "east"]
# AllShapes.SIX_VOXEL_POLE
POLE = box(5, 0, 5, 11, 16, 11)
# AllShapes.SMALL_GEAR / LARGE_GEAR discs (the large one clamped to its block).
SMALL_GEAR = box(2, 6, 2, 14, 10, 14)
LARGE_GEAR = box(0, 6, 0, 16, 10, 16)
# AllShapes.CRANK
CRANK = box(5, 0, 5, 11, 6, 11)

# Rotating blocks: drawn by the native mod, hidden from the chunk mesh.
ROTATING = {
    "shaft": {"shapes": [POLE], "particle": "create:block/axis"},
    "cogwheel": {"shapes": [POLE, SMALL_GEAR], "particle": "block/stripped_spruce_log_top"},
    "large_cogwheel": {"shapes": [POLE, LARGE_GEAR], "particle": "block/stripped_spruce_log_top"},
}

# Directional kinetic blocks: a template with all six facings and nothing
# else of its own that shows (Create's DirectionalKineticBlock). `turns`:
# the block's own model rotates (KineticBlockEntityRenderer draws it), so it
# leaves the chunk mesh like the rotating blocks.
DIRECTIONAL = {
    "creative_motor": {"template": "minecraft:lightning_rod", "shape": box(0, 0, 0, 16, 16, 16)},
    "hand_crank": {"template": "minecraft:lightning_rod", "shape": CRANK, "turns": "create:block/axis"},
    # Its own model is a static frame; the wheel itself is an OBJ model the
    # native mod draws turning.
    "water_wheel": {"template": "minecraft:lightning_rod", "shape": box(0, 0, 0, 16, 16, 16)},
    # The casing stays in the chunk mesh; the propeller is a partial.
    "encased_fan": {"template": "minecraft:lightning_rod", "shape": box(0, 0, 0, 16, 16, 16)},
    # AllShapes.PUMP; its cog is a partial.
    "mechanical_pump": {"template": "minecraft:lightning_rod", "shape": box(2, 0, 2, 14, 16, 14)},
    # BearingBlock: the casing stays in the chunk mesh; its top turns with
    # the structure (a partial) and its shaft half with the kinetics.
    "mechanical_bearing": {"template": "minecraft:lightning_rod", "shape": box(0, 0, 0, 16, 16, 16)},
    "windmill_bearing": {"template": "minecraft:lightning_rod", "shape": box(0, 0, 0, 16, 16, 16)},
    # DrillBlock: AllShapes.CASING_12PX; the casing stays in the chunk mesh,
    # its head is a partial turning with the kinetics.
    "mechanical_drill": {"template": "minecraft:lightning_rod", "shape": box(0, 0, 0, 16, 12, 16)},
    # SailBlock (a WrenchableDirectionalBlock): AllShapes.SAIL.
    **{name: {"template": "minecraft:lightning_rod", "shape": box(0, 5, 0, 16, 10, 16)}
       for name in ["sail_frame"] + [f"{c}_sail" for c in [
           "white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray", "light_gray", "cyan",
           "purple", "blue", "brown", "green", "red", "black"]]},
}

# Machines with item slots: mod block entities need a template that has a
# block entity. An enchanting table has one, no state properties, and doesn't
# hide its neighbours' faces; its light and its menu are taken away (the
# native mod stops the click). With no property, its shape stays its own
# (12/16 high): the depot is 13/16, the millstone a full block.
CONTAINERS = {
    "depot": {"template": "minecraft:enchanting_table"},
    "millstone": {"template": "minecraft:enchanting_table"},
    # BasinBlock's `facing` (where it pours out) needs a template property:
    # it always faces down, into its own output inventory.
    "basin": {"template": "minecraft:enchanting_table", "model": "create:block/basin/block"},
    # ChuteBlock's `facing` and `shape` need template properties: chutes are
    # straight (facing down), drawn as the plain one.
    "chute": {"template": "minecraft:enchanting_table", "model": "create:block/chute/block"},
    # ArmBlock's `ceiling` needs a template property: arms stand on the floor.
    # The block keeps its base model; the native mod draws the cog and body.
    "mechanical_arm": {"template": "minecraft:enchanting_table", "model": "create:block/mechanical_arm/block"},
}

# NeoForge's convention tags the recipes name and Horizon has no copy of
# (NeoForge 21.1's own data for vanilla's items: `c:ingots/iron` is vanilla's
# iron ingot...). Tags only other mods fill (`c:ingots/lead`) stay empty.
COLORS = ["white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray", "light_gray", "cyan",
          "purple", "blue", "brown", "green", "red", "black"]
WOODS = ["acacia", "birch", "cherry", "dark_oak", "jungle", "mangrove", "oak", "spruce"]
CONVENTION_TAGS = {
    "ingots/iron": ["minecraft:iron_ingot"],
    "ingots/gold": ["minecraft:gold_ingot"],
    "ingots/copper": ["minecraft:copper_ingot"],
    "ingots/netherite": ["minecraft:netherite_ingot"],
    "nuggets/iron": ["minecraft:iron_nugget"],
    "nuggets/gold": ["minecraft:gold_nugget"],
    "barrels/wooden": ["minecraft:barrel"],
    "chests/wooden": ["minecraft:chest", "minecraft:trapped_chest"],
    **{f"dyes/{c}": [f"minecraft:{c}_dye"] for c in COLORS},
    "feathers": ["minecraft:feather"],
    "gunpowders": ["minecraft:gunpowder"],
    "leathers": ["minecraft:leather"],
    "netherracks": ["minecraft:netherrack"],
    "sands/colorless": ["minecraft:sand"],
    "sands/red": ["minecraft:red_sand"],
    "slimeballs": ["minecraft:slime_ball"],
    "storage_blocks/copper": ["minecraft:copper_block"],
    "storage_blocks/iron": ["minecraft:iron_block"],
    "cobblestones": ["minecraft:cobblestone", "minecraft:infested_cobblestone", "minecraft:mossy_cobblestone",
                     "minecraft:cobbled_deepslate"],
    "dusts/redstone": ["minecraft:redstone"],
    "eggs": ["minecraft:egg"],
    "ender_pearls": ["minecraft:ender_pearl"],
    "gems/lapis": ["minecraft:lapis_lazuli"],
    "gems/quartz": ["minecraft:quartz"],
    "glass_blocks": ["minecraft:glass", "minecraft:tinted_glass"] + [f"minecraft:{c}_stained_glass" for c in COLORS],
    "obsidians": ["minecraft:obsidian", "minecraft:crying_obsidian"],
    "raw_materials/copper": ["minecraft:raw_copper"],
    "raw_materials/gold": ["minecraft:raw_gold"],
    "raw_materials/iron": ["minecraft:raw_iron"],
    "rods/wooden": ["minecraft:stick"],
    "stones": ["minecraft:andesite", "minecraft:diorite", "minecraft:granite", "minecraft:stone", "minecraft:deepslate",
               "minecraft:tuff"],
    "storage_blocks/raw_copper": ["minecraft:raw_copper_block"],
    "storage_blocks/raw_gold": ["minecraft:raw_gold_block"],
    "storage_blocks/raw_iron": ["minecraft:raw_iron_block"],
    "strings": ["minecraft:string"],
    "stripped_logs": [f"minecraft:stripped_{w}_log" for w in WOODS] + ["minecraft:stripped_bamboo_block"],
    "stripped_woods": [f"minecraft:stripped_{w}_wood" for w in WOODS],
}

# Horizontal kinetic blocks: a grindstone has a horizontal `facing`, is
# placed facing the player, needs no support and hides no face next to it (Create's
# press is open underneath). The native mod stops its menu.
# AllShapes.MECHANICAL_PROCESSOR_SHAPE: a full block with a 8x8 hole down
# its middle.
PROCESSOR = [box(0, 0, 0, 4, 16, 16), box(12, 0, 0, 16, 16, 16), box(4, 0, 0, 12, 16, 4), box(4, 0, 12, 12, 16, 16)]

HORIZONTAL = {
    "mechanical_press": {"template": "minecraft:grindstone", "shapes": [box(0, 0, 0, 16, 16, 16)]},
    # The mixer has no state of its own: a grindstone hosts it as it does
    # the press (its menu is stopped), whatever way it faces.
    "mechanical_mixer": {"template": "minecraft:grindstone", "shapes": PROCESSOR},
}

# AllPartialModels the renderers draw: hidden blocks holding each model
# untransformed, which the native mod turns to face their way.
PARTIALS = {
    "shaft_half": "create:block/shaft_half",
    "hand_crank_handle": "create:block/hand_crank/handle",
    "press_head": "create:block/mechanical_press/head",
    "millstone_inner": "create:block/millstone/inner",
    "encased_fan_inner": "create:block/encased_fan/propeller",
    "cogwheel_shaftless": "create:block/cogwheel_shaftless",
    "mechanical_mixer_pole": "create:block/mechanical_mixer/pole",
    "mechanical_mixer_head": "create:block/mechanical_mixer/head",
    # PumpRenderer's cog.
    "mechanical_pump_cog": "create:block/mechanical_pump/cog",
    # ArmRenderer's and DeployerRenderer's partials.
    "arm_cog": "create:block/mechanical_arm/cog",
    "arm_base": "create:block/mechanical_arm/base",
    "arm_lower_body": "create:block/mechanical_arm/lower_body",
    "arm_upper_body": "create:block/mechanical_arm/upper_body",
    "arm_claw_base": "create:block/mechanical_arm/claw_base",
    "arm_upper_claw_grip": "create:block/mechanical_arm/upper_claw_grip",
    "arm_lower_claw_grip": "create:block/mechanical_arm/lower_claw_grip",
    "deployer_pole": "create:block/deployer/pole",
    "deployer_hand_pointing": "create:block/deployer/hand_pointing",
    "deployer_hand_punching": "create:block/deployer/hand_punching",
    "deployer_hand_holding": "create:block/deployer/hand_holding",
    # PulleyRenderer's coil and rope ends.
    "rope_coil": "create:block/rope_pulley/rope_coil",
    "rope_half": "create:block/rope_pulley/rope_half",
    "rope_half_magnet": "create:block/rope_pulley/rope_half_magnet",
    # DrillRenderer's head.
    "drill_head": "create:block/mechanical_drill/head",
    # GantryCarriageRenderer's wheels.
    "gantry_carriage_wheels": "create:block/gantry_carriage/wheels",
    # BearingRenderer's tops.
    "bearing_top": "create:block/bearing/top",
    "bearing_top_wooden": "create:block/bearing/top_wooden",
    # BeltRenderer's partials.
    "belt_pulley": "create:block/belt_pulley",
    "belt_start": "create:block/belt/start",
    "belt_middle": "create:block/belt/middle",
    "belt_end": "create:block/belt/end",
    "belt_start_bottom": "create:block/belt/start_bottom",
    "belt_middle_bottom": "create:block/belt/middle_bottom",
    "belt_end_bottom": "create:block/belt/end_bottom",
    "belt_diagonal_start": "create:block/belt/diagonal_start",
    "belt_diagonal_middle": "create:block/belt/diagonal_middle",
    "belt_diagonal_end": "create:block/belt/diagonal_end",
}

# Belts: Create's one `create:belt` (facing, slope, part, casing) is a block
# per part here, on a grindstone's template: it has a horizontal `facing`
# and a `face` (floor, wall, ceiling) that holds the slope (horizontal,
# upward, downward), needs no support and has no use the native mod doesn't
# stop. Vertical and sideways belts have no block. Shapes can only follow one
# property: the flat belt's slab (both ways) and a lower half for slopes.
BELT_PARTS = {"belt": "start", "hz_belt_middle": "middle", "hz_belt_end": "end", "hz_belt_pulley": "pulley"}
BELT_SHAPES = {
    "floor": [box(0, 3, 0, 16, 13, 16)],
    "wall": [box(0, 0, 0, 16, 8, 16)],
    "ceiling": [box(0, 0, 0, 16, 8, 16)],
}

# Funnels: a lightning rod's template (six facings), a block for each
# `extracting`; the item places the extracting one. AllShapes.FUNNEL_*,
# within the block.
FUNNEL_KINDS = {
    "andesite_funnel": {"andesite_funnel": "true", "hz_andesite_funnel_collecting": "false"},
    "brass_funnel": {"brass_funnel": "true", "hz_brass_funnel_collecting": "false"},
}
FUNNEL_WALL_NORTH = [box(2, 2, 14, 14, 14, 16), box(1, 8, 12, 15, 15, 14), box(0.1, 13, 7, 15.9, 15, 11),
                     box(0.1, 9, 8, 15.9, 13, 12), box(0.1, 5, 9, 15.9, 9, 13)]
FUNNEL_SHAPES = {
    "up": [box(2, 0, 2, 14, 8, 14), box(1, 1, 1, 15, 8, 15), box(0, 4, 0, 16, 10, 16)],
    "down": [box(2, 8, 2, 14, 16, 14), box(1, 8, 1, 15, 15, 15), box(0, 6, 0, 16, 12, 16)],
}

# Belt tunnels: Create's `horizontal_axis` and `shape` are a chain's
# template (its `axis`) and a block per shape; the item places the straight
# one and the native mod turns it to the axis and shape its belt and
# neighbours give it. BeltTunnelShapes (a block with the tunnel's openings
# notched out at both ends, its part below the block left out) along `axis`
# only: shapes can't follow the block per shape.
TUNNEL_SHAPES = ["straight", "window", "closed", "t_left", "t_right", "cross"]
TUNNEL_ALONG_Z = [box(0, 0, 0, 2, 16, 16), box(14, 0, 0, 16, 16, 16), box(2, 10, 0, 14, 16, 16),
                  box(2, 0, 2, 14, 10, 14)]


def tunnel_blocks(kind):
    return {(f"{kind}_tunnel" if shape == "straight" else f"hz_{kind}_tunnel_{shape}"): shape
            for shape in TUNNEL_SHAPES}


# The deployer: a lightning rod's template (six facings, no use of its own), a
# block per `axis_along_first` (no template has it); the item places the
# first, the native mod picks the right one. AllShapes.CASING_12PX.
DEPLOYERS = {"deployer": "false", "hz_deployer_along": "true"}
CASING_12PX = box(0, 0, 0, 16, 12, 16)

# The item vault: a barrel's template (a block entity for its slots, a full
# block, a `facing` holding its horizontal axis: east for X, south for Z).
VAULT_TURNS = {"east": 90, "west": 90, "north": 0, "south": 0, "up": 0, "down": 0}

# AllPartialModels.WATER_WHEEL, an OBJ model (`neoforge:obj`) with its
# materials and texture parents: read by the native mod at runtime.
OBJ_FILES = [
    "assets/create/models/block/water_wheel/wheel.json",
    "assets/create/models/block/water_wheel/textures.json",
    "assets/create/models/block/water_wheel/water_wheel.obj",
    "assets/create/models/block/water_wheel/water_wheel.mtl",
    "assets/create/models/block/large_wheels.json",
]


def read_json(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        json.dump(value, f, indent=2)
        f.write("\n")


def carry(jar, resources, port_pack, work):
    """Runs horizon-port-pack on the jar alone; returns the carried folder."""
    jars = work / "pack"
    jars.mkdir(parents=True, exist_ok=True)
    shutil.copy2(jar, jars / jar.name)
    run = lambda *a: subprocess.run([str(port_pack), *a], check=True)
    run("import", str(jars), str(work / "port"))
    (work / "port" / "jars").mkdir(exist_ok=True)
    shutil.copy2(jar, work / "port" / "jars" / jar.name)
    run("inventory", str(work / "port"), "--offline")
    run("carry", str(work / "port"), "--resources", str(resources), "create")
    return work / "port" / "mods" / "create" / "carry"


def fit(content):
    blocks = content / "data" / "create" / "horizon" / "block"
    states = content / "assets" / "create" / "blockstates"
    models = content / "assets" / "create" / "models" / "block"
    lang = content / "assets" / "create" / "lang" / "en_us.json"
    names = read_json(lang) if lang.exists() else {}

    for name, spec in ROTATING.items():
        decl = read_json(blocks / f"{name}.json")
        # A chain: an `axis`, placed like a log, but it neither occludes its
        # neighbours' faces nor darkens its own cell, as Create's shafts.
        decl["template"] = "minecraft:iron_chain"
        decl["shapes"] = {
            "property": "axis",
            "values": {a: [along(a, b) for b in spec["shapes"]] for a in AXES},
        }
        write_json(blocks / f"{name}.json", decl)
        # The part the native mod draws: the block's own blockstate, kept
        # under a hidden block.
        original = read_json(states / f"{name}.json")
        write_json(states / f"hz_{name}.json", original)
        write_json(blocks / f"hz_{name}.json", {"template": "minecraft:iron_chain", "item": False})
        names[f"block.create.hz_{name}"] = names.get(f"block.create.{name}", name)
        # The block itself shows nothing but its particles.
        write_json(models / f"hz_blank_{name}.json", {"textures": {"particle": spec["particle"]}})
        write_json(states / f"{name}.json", {
            "variants": {f"axis={a}": {"model": f"create:block/hz_blank_{name}"} for a in AXES}
        })

    for name, spec in DIRECTIONAL.items():
        decl = read_json(blocks / f"{name}.json")
        decl["template"] = spec["template"]
        decl.pop("default_state", None)
        decl["shapes"] = {
            "property": "facing",
            "values": {f: [facing_box(f, spec["shape"])] for f in FACINGS},
        }
        props = decl.setdefault("properties", {})
        props["light"] = 0
        write_json(blocks / f"{name}.json", decl)
        if "turns" in spec:
            original = read_json(states / f"{name}.json")
            write_json(states / f"hz_{name}.json", original)
            write_json(blocks / f"hz_{name}.json", {"template": spec["template"], "item": False})
            names[f"block.create.hz_{name}"] = names.get(f"block.create.{name}", name)
            write_json(models / f"hz_blank_{name}.json", {"textures": {"particle": spec["turns"]}})
            write_json(states / f"{name}.json", {
                "variants": {f"facing={f}": {"model": f"create:block/hz_blank_{name}"} for f in FACINGS}
            })

    for name, spec in HORIZONTAL.items():
        decl = read_json(blocks / f"{name}.json")
        decl["template"] = spec["template"]
        decl["default_state"] = {"face": "floor"}
        decl["shapes"] = {
            "property": "facing",
            "values": {f: spec["shapes"] for f in ["north", "south", "west", "east"]},
        }
        write_json(blocks / f"{name}.json", decl)

    for name, spec in CONTAINERS.items():
        # horizon-port-pack leaves out blocks no template fits (the arm's
        # `ceiling`): declare those here.
        decl = read_json(blocks / f"{name}.json") if (blocks / f"{name}.json").exists() else {}
        decl["template"] = spec["template"]
        decl.pop("default_state", None)
        decl.pop("shapes", None)
        decl.setdefault("properties", {})["light"] = 0
        write_json(blocks / f"{name}.json", decl)
        if "model" in spec:
            write_json(states / f"{name}.json", {"variants": {"": {"model": spec["model"]}}})

    # Horizon reads `data/<ns>/recipe/*.json` only, not its subfolders, where
    # vanilla reads them all (`create:pressing/iron_ingot`): bring each up as
    # `create:pressing.iron_ingot`. 1198 of Create's recipes, crafting
    # included, would be missing otherwise.
    for recipe_dir in (content / "data").glob("*/recipe"):
        for path in sorted(recipe_dir.rglob("*.json")):
            rel = path.relative_to(recipe_dir)
            if len(rel.parts) == 1:
                continue
            flat = recipe_dir / ".".join(rel.parts)
            path.rename(flat)
        for sub in sorted((d for d in recipe_dir.rglob("*") if d.is_dir()), reverse=True):
            if not any(sub.iterdir()):
                sub.rmdir()

    tags = content / "data" / "c" / "tags" / "item"
    for tag, items in CONVENTION_TAGS.items():
        path = tags / f"{tag}.json"
        existing = read_json(path)["values"] if path.exists() else []
        write_json(path, {"values": sorted(set(existing) | set(items))})

    for name, model in PARTIALS.items():
        block = f"hz_part_{name}"
        write_json(blocks / f"{block}.json", {"template": "minecraft:stone", "item": False})
        write_json(states / f"{block}.json", {"variants": {"": {"model": model}}})
        names[f"block.create.{block}"] = name

    fit_logistics(content, names)
    fit_fluids(content)

    if names:
        write_json(lang, names)


def turn_y(b, quarter_turns):
    """A box turned clockwise about Y (seen from above) in quarter turns."""
    x0, y0, z0, x1, y1, z1 = b
    for _ in range(quarter_turns % 4):
        x0, z0, x1, z1 = 1 - z1, x0, 1 - z0, x1
    return [x0, y0, z0, x1, y1, z1]


def loot(name, items):
    # In 26.3's format (as horizon-port-pack upgrades the carried tables):
    # one `condition` object with a `type`, not 1.21.1's `conditions` list.
    return {
        "type": "minecraft:block",
        "pools": [{"rolls": 1.0, "bonus_rolls": 0.0, "condition": {"type": "minecraft:survives_explosion"},
                   "entries": [{"type": "minecraft:item", "name": item}]} for item in items],
        "random_sequence": f"create:blocks/{name}",
    }


def fit_logistics(content, names):
    blocks = content / "data" / "create" / "horizon" / "block"
    states = content / "assets" / "create" / "blockstates"
    loot_tables = content / "data" / "create" / "loot_table" / "blocks"
    base = read_json(blocks / "belt.json") if (blocks / "belt.json").exists() else {}
    for name, part in BELT_PARTS.items():
        decl = dict(base)
        decl["template"] = "minecraft:grindstone"
        decl["item"] = False
        decl["default_state"] = {"face": "floor"}
        decl["shapes"] = {"property": "face", "values": BELT_SHAPES}
        decl.setdefault("properties", {})["light"] = 0
        write_json(blocks / f"{name}.json", decl)
        # BeltBlock's blockstate shows only particles; the native mod draws it.
        write_json(states / f"{name}.json", {
            "variants": {f"face={f}": {"model": "create:block/belt/particle"} for f in BELT_SHAPES}
        })
        # The belt drops its connector, and a shaft where it had a pulley
        # (`BeltBlock.getDrops`).
        items = ["create:belt_connector"] + ([] if part == "middle" else ["create:shaft"])
        write_json(loot_tables / f"{name}.json", loot(name, items))
        if name != "belt":
            names[f"block.create.{name}"] = names.get("block.create.belt", "Mechanical Belt")

    turns = {"north": 0, "east": 1, "south": 2, "west": 3}
    shapes = dict(FUNNEL_SHAPES)
    for f, q in turns.items():
        shapes[f] = [turn_y(b, q) for b in FUNNEL_WALL_NORTH]
    for item, funnels in FUNNEL_KINDS.items():
        original = read_json(states / f"{item}.json") if (states / f"{item}.json").exists() else None
        base = read_json(blocks / f"{item}.json") if (blocks / f"{item}.json").exists() else {}
        for name, extracting in funnels.items():
            decl = dict(base)
            decl["template"] = "minecraft:lightning_rod"
            decl.pop("default_state", None)
            decl["shapes"] = {"property": "facing", "values": shapes}
            decl.setdefault("properties", {})["light"] = 0
            if name != item:
                decl["item"] = False
                names[f"block.create.{name}"] = names.get(f"block.create.{item}", item.replace("_", " ").title())
            if name != "andesite_funnel":
                write_json(loot_tables / f"{name}.json", loot(name, [f"create:{item}"]))
            write_json(blocks / f"{name}.json", decl)
            if original is None:
                continue
            variants = {}
            for key, model in original.get("variants", {}).items():
                props = dict(kv.split("=", 1) for kv in key.split(",") if "=" in kv)
                if props.get("extracting") == extracting and props.get("powered", "false") == "false" \
                        and props.get("waterlogged", "false") == "false" and "facing" in props:
                    variants[f"facing={props['facing']}"] = model
            write_json(states / f"{name}.json", {"variants": variants})

    tunnel_shapes = {"z": TUNNEL_ALONG_Z}
    tunnel_shapes["x"] = [[z0, y0, x0, z1, y1, x1] for x0, y0, z0, x1, y1, z1 in TUNNEL_ALONG_Z]
    tunnel_shapes["y"] = tunnel_shapes["x"]
    for kind in ("andesite", "brass"):
        item = f"{kind}_tunnel"
        original = read_json(states / f"{item}.json") if (states / f"{item}.json").exists() else None
        base = read_json(blocks / f"{item}.json") if (blocks / f"{item}.json").exists() else {}
        for name, shape in tunnel_blocks(kind).items():
            decl = dict(base)
            decl["template"] = "minecraft:iron_chain"
            decl.pop("default_state", None)
            decl["shapes"] = {"property": "axis", "values": tunnel_shapes}
            decl.setdefault("properties", {})["light"] = 0
            if name != item:
                decl["item"] = False
                names[f"block.create.{name}"] = names.get(f"block.create.{item}", item.replace("_", " ").title())
            write_json(blocks / f"{name}.json", decl)
            write_json(loot_tables / f"{name}.json", loot(name, [f"create:{item}"]))
            if original is None:
                continue
            variants = {}
            for key, model in original.get("variants", {}).items():
                props = dict(kv.split("=", 1) for kv in key.split(",") if "=" in kv)
                if props.get("shape") == shape and props.get("axis") in ("x", "z"):
                    variants[f"axis={props['axis']}"] = model
            # A chain placed upright, until the native mod turns it.
            if "axis=x" in variants:
                variants["axis=y"] = variants["axis=x"]
            write_json(states / f"{name}.json", {"variants": variants})

    fit_arms(content, names)
    fit_pistons(content, names)
    fit_chassis(content, names)
    fit_rope(content)
    fit_gantry(content, names)
    fit_actors(content)

    # The extra blocks share their base block's tags: `mineable/*` decides
    # whether the template's required tool drops them at all.
    share_block_tags(content, {
        **{name: "create:belt" for name in BELT_PARTS if name != "belt"},
        **{name: f"create:{item}" for item, funnels in FUNNEL_KINDS.items() for name in funnels if name != item},
        **{name: f"create:{kind}_tunnel" for kind in ("andesite", "brass")
           for name in tunnel_blocks(kind) if name != f"{kind}_tunnel"},
        **{name: "create:deployer" for name in DEPLOYERS if name != "deployer"},
        **{name: f"create:{piston}" for name, (piston, _, _) in piston_blocks().items() if name != piston},
        "hz_mechanical_piston_head_sticky": "create:mechanical_piston_head",
        **{name: f"create:{base}" for name, (base, _) in chassis_blocks().items() if name != base},
        **{f"hz_gantry_shaft_{part}": "create:gantry_shaft" for part in ("start", "middle", "end")},
        "hz_gantry_carriage_along": "create:gantry_carriage",
    })


# Fluid pipes: Create's `fluid_pipe` (six `north`...`down` booleans, set by
# its neighbours) is straight here, on a chain's template: `axis` from the
# clicked face, open at both ends. FluidPipeBlock's core and arms
# (`PipeBlock` with an apothem of 4/16).
PIPE_SHAPE = box(4, 0, 4, 12, 16, 12)
SIDES = {"x": ("west", "east"), "y": ("down", "up"), "z": ("north", "south")}


def matches(when, props):
    """A multipart `when` (with `OR`/`AND` and `a|b` values) against `props`."""
    if "OR" in when:
        return any(matches(w, props) for w in when["OR"])
    if "AND" in when:
        return all(matches(w, props) for w in when["AND"])
    return all(props.get(k) in str(v).split("|") for k, v in when.items())


def straight_pipe_states(original):
    """The carried six-way pipe blockstate, for straight pipes by `axis`."""
    parts = []
    for axis in AXES:
        props = {d: "false" for d in FACINGS}
        props.update({d: "true" for d in SIDES[axis]})
        props["waterlogged"] = "false"
        for part in original.get("multipart", []):
            if "when" not in part or matches(part["when"], props):
                parts.append({"when": {"axis": axis}, "apply": part["apply"]})
    return {"multipart": parts}


def fit_fluids(content):
    blocks = content / "data" / "create" / "horizon" / "block"
    states = content / "assets" / "create" / "blockstates"
    pipe = blocks / "fluid_pipe.json"
    decl = read_json(pipe) if pipe.exists() else {}
    decl["template"] = "minecraft:iron_chain"
    decl.pop("default_state", None)
    decl["shapes"] = {"property": "axis", "values": {a: [along(a, PIPE_SHAPE)] for a in AXES}}
    decl.setdefault("properties", {})["light"] = 0
    write_json(pipe, decl)
    if (states / "fluid_pipe.json").exists():
        write_json(states / "fluid_pipe.json", straight_pipe_states(read_json(states / "fluid_pipe.json")))
    # The fluid tank: a single block on glass's template (a full block that
    # hides nothing next to it, cutout), with no block entity: its fluid
    # lives in the native mod. Single tanks only: always the one-block
    # window model (FluidTankBlock's `top`, `bottom`, `shape`).
    tank = blocks / "fluid_tank.json"
    decl = read_json(tank) if tank.exists() else {}
    decl["template"] = "minecraft:glass"
    decl.pop("default_state", None)
    decl.pop("shapes", None)
    decl.setdefault("properties", {})["light"] = 0
    write_json(tank, decl)
    write_json(states / "fluid_tank.json", {"variants": {"": {"model": "create:block/fluid_tank/block_single_window"}}})
def fit_arms(content, names):
    """The item vault, and the deployer's blocks (the arm is a container)."""
    blocks = content / "data" / "create" / "horizon" / "block"
    states = content / "assets" / "create" / "blockstates"
    loot_tables = content / "data" / "create" / "loot_table" / "blocks"
    decl = read_json(blocks / "item_vault.json") if (blocks / "item_vault.json").exists() else {}
    decl["template"] = "minecraft:barrel"
    decl.pop("default_state", None)
    decl.pop("shapes", None)
    decl.setdefault("properties", {})["light"] = 0
    write_json(blocks / "item_vault.json", decl)
    write_json(states / "item_vault.json", {"variants": {
        f"facing={f}": ({"model": "create:block/item_vault", "y": y} if y else {"model": "create:block/item_vault"})
        for f, y in VAULT_TURNS.items()
    }})
    original = read_json(states / "deployer.json") if (states / "deployer.json").exists() else None
    base = read_json(blocks / "deployer.json") if (blocks / "deployer.json").exists() else {}
    for name, along in DEPLOYERS.items():
        decl = dict(base)
        decl["template"] = "minecraft:lightning_rod"
        decl.pop("default_state", None)
        decl["shapes"] = {"property": "facing", "values": {f: [facing_box(f, CASING_12PX)] for f in FACINGS}}
        decl.setdefault("properties", {})["light"] = 0
        if name != "deployer":
            decl["item"] = False
            names[f"block.create.{name}"] = names.get("block.create.deployer", "Deployer")
            write_json(loot_tables / f"{name}.json", loot(name, ["create:deployer"]))
        write_json(blocks / f"{name}.json", decl)
        if original is None:
            continue
        variants = {}
        for key, model in original.get("variants", {}).items():
            props = dict(kv.split("=", 1) for kv in key.split(",") if "=" in kv)
            if props.get("axis_along_first") == along and "facing" in props:
                variants[f"facing={props['facing']}"] = model
        write_json(states / f"{name}.json", {"variants": variants})


# Mechanical pistons: Create's `facing`, `axis_along_first` and `state` (no
# template has all three): a lightning rod's template (`facing`) per
# piston, `axis_along_first` and state, the plain one the retracted piston
# not along first. Poles and heads are lightning rods too (all six facings).
PISTON_STATES = ["retracted", "moving", "extended"]
FOUR_VOXEL_POLE = box(6, 0, 6, 10, 16, 10)


def piston_blocks():
    """{block: (piston, axis_along_first, state)}, as contraption.rs names them."""
    out = {}
    for piston in ("mechanical_piston", "sticky_mechanical_piston"):
        for along_first in ("false", "true"):
            for state in PISTON_STATES:
                plain = along_first == "false" and state == "retracted"
                name = piston if plain else f"hz_{piston}_{'along' if along_first == 'true' else 'across'}_{state}"
                out[name] = (piston, along_first, state)
    return out


def fit_pistons(content, names):
    blocks = content / "data" / "create" / "horizon" / "block"
    states = content / "assets" / "create" / "blockstates"
    loot_tables = content / "data" / "create" / "loot_table" / "blocks"

    def declare(name, shape, base, item=True):
        decl = read_json(blocks / f"{base}.json") if (blocks / f"{base}.json").exists() else {}
        decl = dict(decl)
        decl["template"] = "minecraft:lightning_rod"
        decl.pop("default_state", None)
        decl["shapes"] = {"property": "facing", "values": {f: [facing_box(f, b) for b in shape] for f in FACINGS}}
        decl.setdefault("properties", {})["light"] = 0
        if not item:
            decl["item"] = False
            names[f"block.create.{name}"] = names.get(f"block.create.{base}", base)
            write_json(loot_tables / f"{name}.json", loot(name, [f"create:{base}"]))
        write_json(blocks / f"{name}.json", decl)

    def variants(original, keep):
        out = {}
        for key, model in original.get("variants", {}).items():
            props = dict(kv.split("=", 1) for kv in key.split(",") if "=" in kv)
            if keep(props) and "facing" in props:
                out[f"facing={props['facing']}"] = model
        return out

    full = [box(0, 0, 0, 16, 16, 16)]
    # The carried blockstates, read before the plain blocks' are rewritten.
    originals = {n: read_json(states / f"{n}.json") for n in
                 ("mechanical_piston", "sticky_mechanical_piston", "mechanical_piston_head")
                 if (states / f"{n}.json").exists()}
    for name, (piston, along_first, state) in piston_blocks().items():
        shape = full if state == "retracted" else [CASING_12PX, FOUR_VOXEL_POLE]
        declare(name, shape, piston, item=name == piston)
        if piston in originals:
            original = originals[piston]
            write_json(states / f"{name}.json", {"variants": variants(
                original, lambda p: p.get("axis_along_first") == along_first and p.get("state") == state)})
    declare("piston_extension_pole", [FOUR_VOXEL_POLE], "piston_extension_pole")
    head = [box(0, 12, 0, 16, 16, 16), FOUR_VOXEL_POLE]
    for kind in ("normal", "sticky"):
        name = "mechanical_piston_head" if kind == "normal" else "hz_mechanical_piston_head_sticky"
        declare(name, head, "mechanical_piston_head", item=False)
        if "mechanical_piston_head" in originals:
            original = originals["mechanical_piston_head"]
            write_json(states / f"{name}.json", {"variants": variants(original, lambda p: p.get("type") == kind)})
    # The head drops nothing (Create: its loot table is empty): the piston
    # keeps it.
    write_json(loot_tables / "mechanical_piston_head.json", {"type": "minecraft:block", "pools": []})
    write_json(loot_tables / "hz_mechanical_piston_head_sticky.json", {"type": "minecraft:block", "pools": []})


# Chassis (`AbstractChassisBlock`, a RotatedPillarBlock): a log's template
# keeps `axis`; each set of sticky sides is a block of its own,
# `create:hz_<kind>_<sides>` (contraption.rs's `chassis_block`), dropping the
# plain one. The bits follow `ChassisKind::props`.
CHASSIS_SIDES = {
    "linear_chassis": ["top", "bottom"],
    "secondary_linear_chassis": ["top", "bottom"],
    "radial_chassis": ["north", "east", "south", "west"],
}


def chassis_blocks():
    """Every chassis block: name -> (base, {property: "true"/"false"})."""
    out = {}
    for base, sides in CHASSIS_SIDES.items():
        for bits in range(1 << len(sides)):
            on = [side for i, side in enumerate(sides) if bits & (1 << i)]
            name = base if not on else f"hz_{base}_{'_'.join(on)}"
            out[name] = (base, {f"sticky_{side}": str(side in on).lower() for side in sides})
    return out


def fit_chassis(content, names):
    blocks = content / "data" / "create" / "horizon" / "block"
    states = content / "assets" / "create" / "blockstates"
    loot_tables = content / "data" / "create" / "loot_table" / "blocks"
    originals = {b: read_json(states / f"{b}.json") for b in CHASSIS_SIDES if (states / f"{b}.json").exists()}
    for name, (base, fixed) in chassis_blocks().items():
        decl = read_json(blocks / f"{base}.json") if (blocks / f"{base}.json").exists() else {}
        decl = dict(decl)
        decl["template"] = "minecraft:oak_log"
        decl.pop("default_state", None)
        decl.pop("shapes", None)
        if name != base:
            decl["item"] = False
            names[f"block.create.{name}"] = names.get(f"block.create.{base}", base)
            write_json(loot_tables / f"{name}.json", loot(name, [f"create:{base}"]))
        write_json(blocks / f"{name}.json", decl)
        original = originals.get(base)
        if original is None:
            continue
        if "variants" in original:
            variants = {}
            for key, model in original["variants"].items():
                props = dict(kv.split("=", 1) for kv in key.split(",") if "=" in kv)
                if all(props.get(k) == v for k, v in fixed.items()) and "axis" in props:
                    variants[f"axis={props['axis']}"] = model
            write_json(states / f"{name}.json", {"variants": variants})
        else:
            parts = []
            for part in original.get("multipart", []):
                when = dict(part.get("when", {}))
                if any(k in fixed and str(v) != fixed[k] for k, v in when.items()):
                    continue
                when = {k: v for k, v in when.items() if k not in fixed}
                parts.append({**part, "when": when} if when else {"apply": part["apply"]})
            write_json(states / f"{name}.json", {"multipart": parts})


# The rope pulley's rope and magnet (no items: the pulley lays them): a
# lightning rod's template standing up, for their shapes (RopeBlock's
# FOUR_VOXEL_POLE, MagnetBlock's PULLEY_MAGNET).
ROPE_SHAPES = {
    "rope": [box(6, 0, 6, 10, 16, 10)],
    "pulley_magnet": [box(3, 0, 3, 13, 2, 13), box(6, 0, 6, 10, 16, 10)],
}


def fit_rope(content):
    blocks = content / "data" / "create" / "horizon" / "block"
    states = content / "assets" / "create" / "blockstates"
    for name, shape in ROPE_SHAPES.items():
        decl = read_json(blocks / f"{name}.json") if (blocks / f"{name}.json").exists() else {}
        decl["template"] = "minecraft:lightning_rod"
        decl["default_state"] = {"facing": "up"}
        decl["shapes"] = {"property": "facing", "values": {f: [facing_box("up", b) for b in shape] for f in FACINGS}}
        decl.setdefault("properties", {})["light"] = 0
        write_json(blocks / f"{name}.json", decl)
        model = f"create:block/rope_pulley/{name}"
        write_json(states / f"{name}.json", {"variants": {f"facing={f}": {"model": model} for f in FACINGS}})


# Gantries. GantryShaftBlock's `part` (start, middle, end, single) makes a
# block each on a lightning rod's template (`facing`; its `powered` is the
# lightning's, so shafts are never powered here): `create:gantry_shaft` alone,
# `create:hz_gantry_shaft_<part>` in a line (contraption.rs's
# `gantry_shaft_block`). Their model turns (ENTITYBLOCK_ANIMATED): the
# blocks show particles only and hidden `create:hz_gantry_shaft_holder_<part>`
# blocks keep the models for the native mod. The carriage is a
# DirectionalAxisKineticBlock, as the deployer.
GANTRY_POLE = box(4, 0, 4, 12, 16, 12)


def fit_gantry(content, names):
    blocks = content / "data" / "create" / "horizon" / "block"
    states = content / "assets" / "create" / "blockstates"
    models = content / "assets" / "create" / "models" / "block"
    loot_tables = content / "data" / "create" / "loot_table" / "blocks"
    originals = {n: read_json(states / f"{n}.json") for n in ("gantry_shaft", "gantry_carriage")
                 if (states / f"{n}.json").exists()}

    def declare(name, base, shape, item):
        decl = read_json(blocks / f"{base}.json") if (blocks / f"{base}.json").exists() else {}
        decl = dict(decl)
        decl["template"] = "minecraft:lightning_rod"
        decl.pop("default_state", None)
        decl["shapes"] = {"property": "facing", "values": {f: [facing_box(f, b) for b in shape] for f in FACINGS}}
        decl.setdefault("properties", {})["light"] = 0
        if not item:
            decl["item"] = False
            names[f"block.create.{name}"] = names.get(f"block.create.{base}", base)
            write_json(loot_tables / f"{name}.json", loot(name, [f"create:{base}"]))
        write_json(blocks / f"{name}.json", decl)

    def variants(original, keep):
        out = {}
        for key, model in original.get("variants", {}).items():
            props = dict(kv.split("=", 1) for kv in key.split(",") if "=" in kv)
            if keep(props) and "facing" in props:
                out[f"facing={props['facing']}"] = model
        return out

    write_json(models / "hz_blank_gantry_shaft.json", {"textures": {"particle": "create:block/gantry_shaft"}})
    blank = {"variants": {f"facing={f}": {"model": "create:block/hz_blank_gantry_shaft"} for f in FACINGS}}
    for part in ("single", "start", "middle", "end"):
        name = "gantry_shaft" if part == "single" else f"hz_gantry_shaft_{part}"
        declare(name, "gantry_shaft", [GANTRY_POLE], item=part == "single")
        write_json(states / f"{name}.json", blank)
        holder = f"hz_gantry_shaft_holder_{part}"
        write_json(blocks / f"{holder}.json", {"template": "minecraft:lightning_rod", "item": False})
        names[f"block.create.{holder}"] = names.get("block.create.gantry_shaft", "gantry_shaft")
        if "gantry_shaft" in originals:
            write_json(states / f"{holder}.json", {"variants": variants(
                originals["gantry_shaft"], lambda p, part=part: p.get("part") == part and p.get("powered") == "false")})
    for along in ("false", "true"):
        name = "gantry_carriage" if along == "false" else "hz_gantry_carriage_along"
        declare(name, "gantry_carriage", [box(0, 0, 0, 16, 16, 16)], item=along == "false")
        if "gantry_carriage" in originals:
            write_json(states / f"{name}.json", {"variants": variants(
                originals["gantry_carriage"], lambda p, along=along: p.get("axis_along_first") == along)})


# AttachedActorBlocks (harvester, plough): a horizontal `facing` on a
# grindstone's template, placed as a wall grindstone (the native mod turns
# one placed on a floor or ceiling); AllShapes.HARVESTER_BASE, modelled here
# on the floor. The harvester's blade, a partial in Create, is drawn with its
# block: it turns only on a moving contraption.
ACTORS = {
    "mechanical_harvester": ["create:block/mechanical_harvester/block", "create:block/mechanical_harvester/blade"],
    "mechanical_plough": ["create:block/mechanical_plough"],
}
HARVESTER_BASE = box(0, 0, 2, 16, 3, 14)
HORIZONTAL_TURNS = {"north": 0, "east": 90, "south": 180, "west": 270}


def fit_actors(content):
    blocks = content / "data" / "create" / "horizon" / "block"
    states = content / "assets" / "create" / "blockstates"
    for name, models in ACTORS.items():
        decl = read_json(blocks / f"{name}.json") if (blocks / f"{name}.json").exists() else {}
        decl["template"] = "minecraft:grindstone"
        decl["default_state"] = {"face": "wall"}
        decl["shapes"] = {
            "property": "facing",
            "values": {f: [facing_box(f, HARVESTER_BASE)] for f in HORIZONTAL_TURNS},
        }
        decl.setdefault("properties", {})["light"] = 0
        write_json(blocks / f"{name}.json", decl)
        write_json(states / f"{name}.json", {"multipart": [
            {"when": {"facing": f}, "apply": {"model": m, "y": turn} if turn else {"model": m}}
            for f, turn in HORIZONTAL_TURNS.items() for m in models
        ]})


def share_block_tags(content, bases):
    """Adds `create:<name>` to every block tag that lists its base."""
    for path in (content / "data").glob("*/tags/block/**/*.json"):
        tag = read_json(path)
        values = tag.get("values", [])
        listed = {v if isinstance(v, str) else v.get("id") for v in values}
        added = [f"create:{n}" for n, base in bases.items() if base in listed and f"create:{n}" not in listed]
        if added:
            tag["values"] = values + added
            write_json(path, tag)


def take_obj_models(jar, out):
    """Copies the OBJ model files the carried content lacks from the
    player's jar (for local use only, as the rest of the content)."""
    import zipfile
    missing = [f for f in OBJ_FILES if not (out / f).exists()]
    if not missing:
        return
    if jar is None:
        print(f"warning: no water wheel model in the content ({', '.join(missing)}); give --jar", file=sys.stderr)
        return
    with zipfile.ZipFile(jar) as z:
        names = set(z.namelist())
        for f in missing:
            if f not in names:
                print(f"warning: {f} is not in {jar.name}", file=sys.stderr)
                continue
            target = out / f
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(z.read(f))


def ingredient_26(ing):
    """A 1.21.1 ingredient as 26.3 writes it (`"x"`, `"#tag"`, or a list),
    as horizon-port-pack upgrades the recipes it carries; None when it has
    a form only NeoForge reads."""
    if isinstance(ing, list):
        parts = [ingredient_26(i) for i in ing]
        return None if any(p is None for p in parts) else parts
    if isinstance(ing, dict) and set(ing) == {"item"}:
        return ing["item"]
    if isinstance(ing, dict) and set(ing) == {"tag"}:
        return "#" + ing["tag"]
    return None


def take_left_out_recipes(jar, out):
    """horizon-port-pack drops the recipes of the blocks it leaves out
    (`recipes_unregistered`); for the ones declared here, carry their
    crafting recipes from the player's jar, upgraded to 26.3's format and
    flattened as `fit` names the others."""
    import zipfile
    blocks = out / "data" / "create" / "horizon" / "block"
    recipes = out / "data" / "create" / "recipe"
    if jar is None:
        print("warning: no --jar: the crafting recipes horizon-port-pack drops aren't carried", file=sys.stderr)
        return
    prefix = "data/create/recipe/"
    with zipfile.ZipFile(jar) as z:
        for name in z.namelist():
            if not (name.startswith(prefix + "crafting/") and name.endswith(".json")):
                continue
            flat = recipes / (name[len(prefix):-len(".json")].replace("/", ".") + ".json")
            if flat.exists():
                continue
            recipe = json.loads(z.read(name))
            if recipe.get("type") not in ("minecraft:crafting_shaped", "minecraft:crafting_shapeless"):
                continue
            result = recipe.get("result", {}).get("id", "")
            if not result.startswith("create:") or not (blocks / f"{result[len('create:'):]}.json").exists():
                continue
            if recipe["type"] == "minecraft:crafting_shaped":
                key = {k: ingredient_26(v) for k, v in recipe["key"].items()}
                if any(v is None for v in key.values()):
                    continue
                recipe["key"] = key
            else:
                ingredients = [ingredient_26(i) for i in recipe["ingredients"]]
                if any(i is None for i in ingredients):
                    continue
                recipe["ingredients"] = ingredients
            write_json(flat, recipe)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--jar", type=Path, help="the player's Create jar (create-1.21.1-6.0.10.jar)")
    parser.add_argument("--carried", type=Path, help="an existing horizon-port-pack carry folder instead of --jar")
    parser.add_argument("--resources", type=Path, help="Horizon's prepared 26.3 resources (for --jar)")
    parser.add_argument("--port-pack", type=Path, default=Path(os.environ.get("HORIZON_PORT_PACK", "horizon-port-pack")))
    parser.add_argument("--out", type=Path, default=ROOT / "target" / "content")
    args = parser.parse_args()
    out = args.out / "create"
    with tempfile.TemporaryDirectory(prefix="create-content-") as tmp:
        if args.carried:
            carried = args.carried
        elif args.jar and args.resources:
            carried = carry(args.jar.resolve(), args.resources.resolve(), args.port_pack, Path(tmp))
        else:
            parser.error("give --jar with --resources, or --carried")
        if out.exists():
            shutil.rmtree(out)
        shutil.copytree(carried, out)
    fit(out)
    take_obj_models(args.jar.resolve() if args.jar else None, out)
    take_left_out_recipes(args.jar.resolve() if args.jar else None, out)
    print(out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
