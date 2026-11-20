//! Самотест `ghost-model-selftest.js` 0.4.9: геометрия модели призрака (MDL 48, VVD, VTX 7),
//! поиск как у игры (custom/*, мод, VPK hl2), путь только внутри мода, битые файлы.

mod support;

use hlsr_delivery::ghost_model as gm;
use std::fs;
use std::path::Path;
use support::scenario::temp_root;

fn put_i32(buf: &mut [u8], value: i32, at: usize) {
    buf[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn put_u16(buf: &mut [u8], value: u16, at: usize) {
    buf[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
fn put_f32(buf: &mut [u8], value: f32, at: usize) {
    buf[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

/// Модель из одного треугольника в форматах Source — те же смещения, что читает `ghost_model`.
fn tiny_model(points: [[f32; 3]; 3]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut mdl = vec![0u8; 240 + 16 + 148 + 116];
    mdl[..4].copy_from_slice(b"IDST");
    put_i32(&mut mdl, 48, 4);
    put_i32(&mut mdl, 1, 232);
    put_i32(&mut mdl, 240, 236); // одна часть тела
    let bp = 240;
    put_i32(&mut mdl, 1, bp + 4);
    put_i32(&mut mdl, 16, bp + 12); // одна модель сразу за частью
    let model = bp + 16;
    put_i32(&mut mdl, 1, model + 72);
    put_i32(&mut mdl, 148, model + 76);
    put_i32(&mut mdl, 0, model + 84);
    let mesh = model + 148;
    put_i32(&mut mdl, 0, mesh + 12);

    let mut vvd = vec![0u8; 64 + 48 * 3];
    vvd[..4].copy_from_slice(b"IDSV");
    put_i32(&mut vvd, 1, 12);
    put_i32(&mut vvd, 3, 16);
    put_i32(&mut vvd, 64, 56);
    for (i, [x, y, z]) in points.into_iter().enumerate() {
        let o = 64 + i * 48 + 16;
        put_f32(&mut vvd, x, o);
        put_f32(&mut vvd, y, o + 4);
        put_f32(&mut vvd, z, o + 8);
    }

    let mut vtx = vec![0u8; 36 + 8 + 8 + 12 + 9 + 25 + 27 + 3 * 9 + 3 * 2];
    put_i32(&mut vtx, 7, 0);
    put_i32(&mut vtx, 1, 28);
    put_i32(&mut vtx, 36, 32);
    let mut o = 36;
    put_i32(&mut vtx, 1, o);
    put_i32(&mut vtx, 8, o + 4); // часть тела -> модель
    o += 8;
    put_i32(&mut vtx, 1, o);
    put_i32(&mut vtx, 8, o + 4); // модель -> LOD
    o += 8;
    put_i32(&mut vtx, 1, o);
    put_i32(&mut vtx, 12, o + 4); // LOD -> сетка
    o += 12;
    put_i32(&mut vtx, 1, o);
    put_i32(&mut vtx, 9, o + 4); // сетка -> группа
    let sg = o + 9;
    let verts = sg + 25 + 27;
    let indices = verts + 27;
    let strip = sg + 25;
    put_i32(&mut vtx, 3, sg);
    put_i32(&mut vtx, (verts - sg) as i32, sg + 4);
    put_i32(&mut vtx, 3, sg + 8);
    put_i32(&mut vtx, (indices - sg) as i32, sg + 12);
    put_i32(&mut vtx, 1, sg + 16);
    put_i32(&mut vtx, (strip - sg) as i32, sg + 20);
    put_i32(&mut vtx, 3, strip);
    put_i32(&mut vtx, 0, strip + 4);
    vtx[strip + 18] = 1; // список треугольников
    for i in 0..3 {
        put_u16(&mut vtx, i as u16, verts + i * 9 + 4);
        put_u16(&mut vtx, i as u16, indices + i * 2);
    }
    (mdl, vvd, vtx)
}

fn put(root: &Path, relative: &str, bytes: &[u8]) {
    let file = relative
        .split('/')
        .fold(root.to_path_buf(), |p, part| p.join(part));
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, bytes).unwrap();
}

fn code<T: std::fmt::Debug>(result: Result<T, gm::GhostModelError>) -> String {
    result.unwrap_err().code
}

#[test]
fn source_model_triangles_from_mdl_vvd_vtx() {
    let (mdl, vvd, vtx) = tiny_model([[0., 0., 0.], [0., 10., 0.], [0., 5., 72.]]);
    let mesh = gm::parse_mesh(&mdl, &vvd, &vtx).unwrap();
    assert_eq!(mesh.triangles, 1);
    let rounded: Vec<f64> = mesh.positions.iter().map(|v| v.round()).collect();
    assert_eq!(rounded, [0., 0., 0., 0., 10., 0., 0., 5., 72.]);
    assert_eq!(code(gm::parse_mesh(b"nope", &vvd, &vtx)), "MODEL_INVALID");
}

#[test]
fn ghost_model_is_found_like_the_game_and_only_inside_the_mod() {
    let (_temp, root) = temp_root();
    let (mdl, vvd, vtx) = tiny_model([[0., 0., 0.], [0., 10., 0.], [0., 5., 72.]]);
    put(&root, "ghostingmod/custom/zz/models/my/ghost.mdl", &mdl);
    put(&root, "ghostingmod/custom/zz/models/my/ghost.vvd", &vvd);
    put(
        &root,
        "ghostingmod/custom/zz/models/my/ghost.dx90.vtx",
        &vtx,
    );
    assert_eq!(
        gm::read_ghost_mesh(&root, Some("models/my/ghost.mdl"))
            .unwrap()
            .triangles,
        1
    );

    // VPK v1 с файлами модели целиком в каталоге (archive 0x7fff).
    let (mdl, vvd, vtx) = tiny_model([[0., 0., 0.], [0., 20., 0.], [0., 10., 80.]]);
    let mut tree: Vec<u8> = Vec::new();
    let mut data: Vec<u8> = Vec::new();
    for (ext, buf) in [("mdl", &mdl), ("vvd", &vvd), ("dx90.vtx", &vtx)] {
        let mut entry = [0u8; 18];
        entry[6..8].copy_from_slice(&0x7fffu16.to_le_bytes());
        entry[8..12].copy_from_slice(&(data.len() as u32).to_le_bytes());
        entry[12..16].copy_from_slice(&(buf.len() as u32).to_le_bytes());
        entry[16..18].copy_from_slice(&0xffffu16.to_le_bytes());
        tree.extend(format!("{ext}\0models/vpk\0gman\0").as_bytes());
        tree.extend(entry);
        tree.extend(b"\0\0");
        data.extend(buf.iter());
    }
    tree.push(0);
    let mut vpk = Vec::new();
    vpk.extend(0x55aa_1234u32.to_le_bytes());
    vpk.extend(1u32.to_le_bytes());
    vpk.extend((tree.len() as u32).to_le_bytes());
    vpk.extend(tree);
    vpk.extend(data);
    put(&root, "hl2/hl2_misc_dir.vpk", &vpk);
    let from_vpk = gm::read_ghost_mesh(&root, Some("models/vpk/gman.mdl")).unwrap();
    assert_eq!(from_vpk.triangles, 1);
    assert_eq!(
        from_vpk.positions.iter().copied().fold(f64::MIN, f64::max),
        80.0
    );

    assert_eq!(
        code(gm::read_ghost_mesh(&root, Some("models/nope.mdl"))),
        "ASSET_NOT_FOUND"
    );
    for bad in [
        "../x.mdl",
        "models/../../x.mdl",
        "C:/x.mdl",
        "models/a.vvd",
        "",
    ] {
        assert_eq!(
            code(gm::read_ghost_mesh(&root, Some(bad))),
            "ASSET_INVALID_PATH",
            "{bad}"
        );
    }
    assert_eq!(code(gm::read_ghost_mesh(&root, None)), "ASSET_INVALID_PATH");
    let picked = root.join("ghostingmod/custom/zz/models/my/ghost.mdl");
    assert_eq!(
        gm::value_of_picked(&root, &picked).unwrap(),
        "models/my/ghost.mdl"
    );
    let own = root.join("ghostingmod").join("models").join("a.mdl");
    assert_eq!(gm::value_of_picked(&root, &own).unwrap(), "models/a.mdl");
    let outside = std::env::temp_dir().join("a.mdl");
    assert_eq!(
        code(gm::value_of_picked(&root, &outside)),
        "ASSET_OUTSIDE_MOD"
    );
}

#[test]
fn broken_model_files_do_not_hang_or_bloat() {
    let (_temp, root) = temp_root();
    // VPK без нуля в конце дерева.
    let mut broken = Vec::new();
    broken.extend(0x55aa_1234u32.to_le_bytes());
    broken.extend(1u32.to_le_bytes());
    broken.extend(4u32.to_le_bytes());
    broken.extend(b"mdlx");
    let vpk = root.join("broken_dir.vpk");
    fs::write(&vpk, broken).unwrap();
    assert_eq!(code(gm::vpk_entry_count(&vpk)), "VPK_INVALID");
    assert_eq!(
        code(gm::vpk_entry_count(&vpk)),
        "VPK_INVALID",
        "сбой запомнен"
    );

    // VVD с fixup на миллиард вершин.
    let (mdl, vvd, vtx) = tiny_model([[0., 0., 0.], [0., 10., 0.], [0., 5., 72.]]);
    let mut bad = vvd.clone();
    bad.extend([0u8; 12]);
    put_i32(&mut bad, 1, 48);
    put_i32(&mut bad, vvd.len() as i32, 52);
    put_i32(&mut bad, 1_000_000_000, vvd.len() + 8);
    assert_eq!(code(gm::parse_mesh(&mdl, &bad, &vtx)), "MODEL_INVALID");
    // Смещения за концом файла — отказ, а не паника.
    assert_eq!(
        code(gm::parse_mesh(&mdl[..250], &vvd, &vtx)),
        "ERR_OUT_OF_RANGE"
    );
}

#[test]
fn triangles_are_capped() {
    // Полоса из 70 000 вершин-индексов по кругу из трёх вершин: треугольников не больше предела.
    let (mdl, vvd, mut vtx) = tiny_model([[0., 0., 0.], [0., 10., 0.], [0., 5., 72.]]);
    let sg = 36 + 8 + 8 + 12 + 9;
    let indices = sg + 25 + 27 + 27;
    let count: i32 = 70_000 * 3;
    vtx.truncate(indices);
    for k in 0..count {
        vtx.extend(((k % 3) as u16).to_le_bytes());
    }
    put_i32(&mut vtx, count, sg + 25);
    let mesh = gm::parse_mesh(&mdl, &vvd, &vtx).unwrap();
    assert_eq!(mesh.triangles, gm::MAX_TRIANGLES);
    assert_eq!(mesh.positions.len(), gm::MAX_TRIANGLES * 9);
}
