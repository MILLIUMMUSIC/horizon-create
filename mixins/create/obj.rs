// Wavefront OBJ models, as NeoForge's `neoforge:obj` loader reads Create's
// (the water wheel's `models/block/water_wheel/wheel.json`): the model JSON
// (its `model`, `flip_v` and textures, parents included), the `.mtl`
// materials (`map_Kd #texture`) and the `.obj` mesh, turned into quads with
// the texture of each. Injected as `crate::game::create_obj`; no Horizon
// types, tested with `cargo test`. The files themselves come from the
// player's Create jar at runtime, never from this repository.

use serde_json::Value;
use std::collections::HashMap;

/// A model JSON's part of an OBJ model.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelJson {
    /// `model`: the `.obj`'s resource id (`create:models/block/x.obj`).
    pub model: Option<String>,
    /// `flip_v`: the OBJ's V runs up, the atlas's down.
    pub flip_v: Option<bool>,
    pub parent: Option<String>,
    pub textures: HashMap<String, String>,
}

/// A model JSON file (`loader`, `model`, `flip_v`, `parent`, `textures`).
pub fn parse_model_json(json: &Value) -> ModelJson {
    let string = |k: &str| json.get(k).and_then(Value::as_str).map(str::to_owned);
    ModelJson {
        model: string("model"),
        flip_v: json.get("flip_v").and_then(Value::as_bool),
        parent: string("parent"),
        textures: json
            .get("textures")
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

impl ModelJson {
    /// Fills in what the child left out from its parent (`textures` merge,
    /// the child's own winning).
    pub fn inherit(&mut self, parent: &ModelJson) {
        if self.model.is_none() {
            self.model = parent.model.clone();
        }
        if self.flip_v.is_none() {
            self.flip_v = parent.flip_v;
        }
        for (k, v) in &parent.textures {
            self.textures.entry(k.clone()).or_insert_with(|| v.clone());
        }
        self.parent = parent.parent.clone();
    }
}

fn split_id(id: &str) -> (&str, &str) {
    id.split_once(':').unwrap_or(("minecraft", id))
}

/// A model id (`create:block/water_wheel/wheel`) as a file of the
/// resources (`assets/create/models/block/water_wheel/wheel.json`).
pub fn model_file(id: &str) -> String {
    let (ns, path) = split_id(id);
    format!("assets/{ns}/models/{path}.json")
}

/// A resource id with its own extension (`create:models/block/x.obj`) as a
/// file of the resources.
pub fn asset_file(id: &str) -> String {
    let (ns, path) = split_id(id);
    format!("assets/{ns}/{path}")
}

/// A texture reference (`#planks`) through `textures`, to a texture id.
pub fn resolve_texture(textures: &HashMap<String, String>, reference: &str) -> Option<String> {
    let mut r = reference.to_owned();
    // Bounded: a cycle of references resolves to nothing.
    for _ in 0..16 {
        match r.strip_prefix('#') {
            Some(name) => r = textures.get(name)?.clone(),
            None => return Some(r),
        }
    }
    None
}

/// A texture id as the block atlas names its sprite: vanilla's without
/// their namespace (`block/oak_planks`), others' with it
/// (`create:block/axis`).
pub fn atlas_key(texture: &str) -> String {
    match texture.strip_prefix("minecraft:") {
        Some(rest) => rest.to_owned(),
        None => texture.to_owned(),
    }
}

/// An `.mtl` file: each material's `map_Kd`.
pub fn parse_mtl(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        let mut words = line.split_whitespace();
        match words.next() {
            Some("newmtl") => current = words.next().map(str::to_owned),
            Some("map_Kd") => {
                if let (Some(m), Some(t)) = (&current, words.last()) {
                    out.insert(m.clone(), t.to_owned());
                }
            }
            _ => {}
        }
    }
    out
}

/// One face corner: indices (0-based) of its position, UV and normal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Corner {
    pub v: usize,
    pub vt: Option<usize>,
    pub vn: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub material: Option<String>,
    pub corners: Vec<Corner>,
}

/// An `.obj` file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Obj {
    pub mtllib: Option<String>,
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub normals: Vec<[f32; 3]>,
    pub faces: Vec<Face>,
}

fn floats<const N: usize>(
    words: &mut std::str::SplitWhitespace<'_>,
    line: usize,
) -> Result<[f32; N], String> {
    let mut out = [0.0; N];
    for v in out.iter_mut() {
        *v = words
            .next()
            .and_then(|w| w.parse().ok())
            .ok_or_else(|| format!("line {line}: a bad number"))?;
    }
    Ok(out)
}

/// An OBJ index (1-based, or negative from the end) as a 0-based one.
fn index(word: &str, len: usize, line: usize) -> Result<usize, String> {
    let i: i64 = word
        .parse()
        .map_err(|_| format!("line {line}: a bad index `{word}`"))?;
    let at = if i > 0 { i - 1 } else { len as i64 + i };
    if at < 0 || at as usize >= len {
        return Err(format!("line {line}: index {i} out of {len}"));
    }
    Ok(at as usize)
}

/// Parses an `.obj` file: `v`, `vt`, `vn`, `f`, `usemtl`, `mtllib`; groups,
/// objects and smoothing are read past.
pub fn parse_obj(text: &str) -> Result<Obj, String> {
    let mut obj = Obj::default();
    let mut material = None;
    for (n, raw) in text.lines().enumerate() {
        let line = n + 1;
        let content = raw.split('#').next().unwrap_or("").trim();
        let mut words = content.split_whitespace();
        match words.next() {
            Some("v") => obj.positions.push(floats::<3>(&mut words, line)?),
            Some("vt") => obj.uvs.push(floats::<2>(&mut words, line)?),
            Some("vn") => obj.normals.push(floats::<3>(&mut words, line)?),
            Some("usemtl") => material = words.next().map(str::to_owned),
            Some("mtllib") => obj.mtllib = words.next().map(str::to_owned),
            Some("f") => {
                let mut corners = Vec::new();
                for w in words {
                    let mut parts = w.split('/');
                    let v = index(parts.next().unwrap_or(""), obj.positions.len(), line)?;
                    let vt = match parts.next() {
                        Some(s) if !s.is_empty() => Some(index(s, obj.uvs.len(), line)?),
                        _ => None,
                    };
                    let vn = match parts.next() {
                        Some(s) if !s.is_empty() => Some(index(s, obj.normals.len(), line)?),
                        _ => None,
                    };
                    corners.push(Corner { v, vt, vn });
                }
                if corners.len() < 3 {
                    return Err(format!("line {line}: a face of {} corners", corners.len()));
                }
                obj.faces.push(Face {
                    material: material.clone(),
                    corners,
                });
            }
            _ => {}
        }
    }
    Ok(obj)
}

/// One quad to draw (a triangle repeats its last corner), in model space
/// (blocks), with UVs across its texture's sprite (0 to 1).
#[derive(Clone, Debug, PartialEq)]
pub struct Quad {
    pub positions: [[f32; 3]; 4],
    pub uvs: [[f32; 2]; 4],
    /// The face's normal (its first corner's, or from its winding).
    pub normal: [f32; 3],
    /// The texture's atlas key.
    pub sprite: String,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l == 0.0 {
        [0.0, 1.0, 0.0]
    } else {
        v.map(|c| c / l)
    }
}

/// The quads of `obj`, each face's texture through its material's
/// `map_Kd` (`materials`), the model's `textures` and then `remap` (the
/// water wheel's planks material), in atlas keys. Faces with no texture are
/// left out; polygons past four corners are fanned into triangles.
pub fn quads(
    obj: &Obj,
    materials: &HashMap<String, String>,
    textures: &HashMap<String, String>,
    flip_v: bool,
    remap: impl Fn(&str) -> String,
) -> Vec<Quad> {
    let mut out = Vec::new();
    for face in &obj.faces {
        let Some(texture) = face
            .material
            .as_ref()
            .and_then(|m| materials.get(m))
            .and_then(|r| resolve_texture(textures, r))
        else {
            continue;
        };
        let sprite = atlas_key(&remap(&texture));
        let corner = |c: &Corner| {
            let p = obj.positions[c.v];
            let mut uv = c.vt.map_or([0.0, 0.0], |i| obj.uvs[i]);
            if flip_v {
                uv[1] = 1.0 - uv[1];
            }
            (p, uv)
        };
        let c = &face.corners;
        let normal = match c[0].vn {
            Some(i) => normalize(obj.normals[i]),
            None => {
                let (a, b, d) = (
                    obj.positions[c[0].v],
                    obj.positions[c[1].v],
                    obj.positions[c[2].v],
                );
                normalize(cross(sub(b, a), sub(d, a)))
            }
        };
        let mut push = |ks: [usize; 4]| {
            let corners = ks.map(|k| corner(&c[k]));
            out.push(Quad {
                positions: corners.map(|(p, _)| p),
                uvs: corners.map(|(_, uv)| uv),
                normal,
                sprite: sprite.clone(),
            });
        };
        match c.len() {
            3 => push([0, 1, 2, 2]),
            4 => push([0, 1, 2, 3]),
            n => {
                for k in 1..n - 1 {
                    push([0, k, k + 1, k + 1]);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const MTL: &str =
        "# made by hand\nnewmtl wood\nmap_Kd #planks\n\nnewmtl iron\nmap_Kd create:block/metal\n";

    const OBJ: &str = "mtllib test.mtl
o Thing
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
vt 0 0
vt 1 0
vt 1 1
vt 0 1
vn 0 0 1
usemtl wood
s 0
f 1/1/1 2/2/1 3/3/1 4/4/1
usemtl iron
f -4/-4 -3/-3 -2/-2
usemtl nothing
f 1 2 3
";

    #[test]
    fn obj_files_parse() {
        let obj = parse_obj(OBJ).unwrap();
        assert_eq!(obj.mtllib.as_deref(), Some("test.mtl"));
        assert_eq!(
            (obj.positions.len(), obj.uvs.len(), obj.normals.len()),
            (4, 4, 1)
        );
        assert_eq!(obj.faces.len(), 3);
        // Negative indices count from the end.
        assert_eq!(
            obj.faces[1].corners[0],
            Corner {
                v: 0,
                vt: Some(0),
                vn: None
            }
        );
        assert!(parse_obj("v 0 0 0\nf 1 2 3\n").is_err());
        assert!(parse_obj("v 0 0\n").is_err());
    }

    #[test]
    fn materials_and_textures_resolve() {
        let materials = parse_mtl(MTL);
        assert_eq!(materials["wood"], "#planks");
        let model = parse_model_json(&json!({
            "parent": "create:block/water_wheel/textures",
            "loader": "neoforge:obj", "flip_v": true,
            "model": "create:models/block/water_wheel/water_wheel.obj"
        }));
        let mut model = model;
        model.inherit(&parse_model_json(&json!({
            "parent": "create:block/large_wheels",
            "textures": {"planks": "block/oak_planks", "particle": "#planks"}
        })));
        assert_eq!(model.flip_v, Some(true));
        assert_eq!(model.parent.as_deref(), Some("create:block/large_wheels"));
        assert_eq!(
            resolve_texture(&model.textures, "#particle").as_deref(),
            Some("block/oak_planks")
        );
        assert_eq!(
            model_file("create:block/water_wheel/wheel"),
            "assets/create/models/block/water_wheel/wheel.json"
        );
        assert_eq!(
            asset_file(model.model.as_deref().unwrap()),
            "assets/create/models/block/water_wheel/water_wheel.obj"
        );
        assert_eq!(atlas_key("minecraft:block/oak_log"), "block/oak_log");
        assert_eq!(atlas_key("create:block/axis"), "create:block/axis");
        let cycle: HashMap<String, String> =
            [("a".into(), "#b".into()), ("b".into(), "#a".into())].into();
        assert_eq!(resolve_texture(&cycle, "#a"), None);
    }

    #[test]
    fn faces_become_quads() {
        let obj = parse_obj(OBJ).unwrap();
        let textures: HashMap<String, String> =
            [("planks".into(), "block/oak_planks".into())].into();
        let remap = |t: &str| t.replace("oak", "spruce");
        let q = quads(&obj, &parse_mtl(MTL), &textures, true, remap);
        // The face with no texture is left out.
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].sprite, "block/spruce_planks");
        assert_eq!(q[0].normal, [0.0, 0.0, 1.0]);
        // flip_v: V turned over.
        assert_eq!(q[0].uvs[0], [0.0, 1.0]);
        assert_eq!(q[0].uvs[2], [1.0, 0.0]);
        // A triangle repeats its last corner; its normal from its winding.
        assert_eq!(q[1].sprite, "create:block/metal");
        assert_eq!(q[1].positions[2], q[1].positions[3]);
        assert_eq!(q[1].normal, [0.0, 0.0, 1.0]);
        // A pentagon fans into three.
        let five =
            parse_obj("v 0 0 0\nv 1 0 0\nv 2 1 0\nv 1 2 0\nv 0 1 0\nusemtl iron\nf 1 2 3 4 5\n")
                .unwrap();
        assert_eq!(
            quads(&five, &parse_mtl(MTL), &textures, false, |t| t.to_owned()).len(),
            3
        );
    }
}
