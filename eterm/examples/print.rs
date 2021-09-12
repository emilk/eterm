//! Print info to help guide what encoding to use for the network.
use egui::epaint;

/// anti_alias gives us around 23% savings in final bandwidth
fn example_output(anti_alias: bool) -> (egui::Output, Vec<egui::ClippedMesh>) {
    let mut ctx = egui::CtxRef::default();
    ctx.memory().options.tessellation_options.anti_alias = anti_alias;

    let raw_input = egui::RawInput::default();
    let mut demo_windows = egui_demo_lib::DemoWindows::default();
    ctx.begin_frame(raw_input);
    demo_windows.ui(&ctx);
    let (output, shapes) = ctx.end_frame();
    let clipped_meshes = ctx.tessellate(shapes);
    (output, clipped_meshes)
}

fn example_shapes() -> (egui::Output, Vec<epaint::ClippedShape>) {
    let mut ctx = egui::CtxRef::default();
    let raw_input = egui::RawInput::default();
    let mut demo_windows = egui_demo_lib::DemoWindows::default();
    ctx.begin_frame(raw_input);
    demo_windows.ui(&ctx);
    ctx.end_frame()
}

fn bincode<S: ?Sized + serde::Serialize>(data: &S) -> Vec<u8> {
    use bincode::Options as _;
    bincode::options().serialize(data).unwrap()
}

fn zstd(data: &[u8], level: i32) -> Vec<u8> {
    zstd::encode_all(std::io::Cursor::new(data), level).unwrap()
}

fn zstd_kb(data: &[u8], level: i32) -> f32 {
    zstd(data, level).len() as f32 * 1e-3
}

// ----------------------------------------------------------------------------

fn print_encodings<S: ?Sized + serde::Serialize>(data: &S) {
    let encoded = bincode(data);
    println!("bincode: {:>6.2} kB", encoded.len() as f32 * 1e-3);
    println!("zstd-0:  {:>6.2} kB", zstd_kb(&encoded, 0));
    println!("zstd-5:  {:>6.2} kB", zstd_kb(&encoded, 5));
    // println!("zstd-15: {:>6.2} kB", zstd_kb(&encoded, 15));
    // println!("zstd-21: {:>6.2} kB (too slow)", zstd_kb(&encoded, 21)); // way too slow
}

fn print_compressions(clipped_meshes: &[egui::ClippedMesh]) {
    let mut num_vertices = 0;
    let mut num_indices = 0;
    let mut bytes_vertices = 0;
    let mut bytes_indices = 0;
    for egui::ClippedMesh(_rect, mesh) in clipped_meshes {
        num_vertices += mesh.vertices.len();
        num_indices += mesh.indices.len();
        bytes_vertices += mesh.vertices.len() * std::mem::size_of_val(&mesh.vertices[0]);
        bytes_indices += mesh.indices.len() * std::mem::size_of_val(&mesh.indices[0]);
    }
    let mesh_bytes = bytes_indices + bytes_vertices;
    println!(
        "vertices: {:>5}  {:>6.2} kb",
        num_vertices,
        bytes_vertices as f32 * 1e-3
    );
    println!(
        "indices:  {:>5}  {:>6.2} kb",
        num_indices,
        bytes_indices as f32 * 1e-3
    );
    println!();

    let net_meshes: Vec<_> = clipped_meshes
        .iter()
        .map(|egui::ClippedMesh(rect, mesh)| (*rect, eterm::net_shape::NetMesh::from(mesh)))
        .collect();

    let mut quantized_meshes = net_meshes.clone();
    for (_, mesh) in &mut quantized_meshes {
        for pos in &mut mesh.pos {
            pos.x = quantize(pos.x);
            pos.y = quantize(pos.y);
        }
    }

    println!("raw:     {:>6.2} kB", mesh_bytes as f32 * 1e-3);
    println!();
    print_encodings(&clipped_meshes);
    println!();
    println!("Flattened mesh:");
    print_encodings(&net_meshes);
    println!();
    println!("Quantized positions:");
    print_encodings(&quantized_meshes);

    // Other things I've tried: delta-encoded positions (5-10% worse).
}

fn main() {
    println!("FontDefinitions:");
    let font_definitions = egui::FontDefinitions::default();
    print_encodings(&font_definitions);
    println!();

    let (_, clipped_meshes) = example_output(true);
    println!("Antialiasing ON:");
    print_compressions(&clipped_meshes);
    println!();

    let (_, clipped_meshes) = example_output(false);
    println!("Antialiasing OFF:");
    print_compressions(&clipped_meshes);
    println!();

    let (_, shapes) = example_shapes();
    let net_shapes = eterm::net_shape::to_clipped_net_shapes(shapes);
    println!("Shapes:");
    print_encodings(&net_shapes);
    println!();
}

fn quantize(f: f32) -> f32 {
    // TODO: should be based on pixels_to_point

    // let precision = 2.0; // 15% wins
    let precision = 8.0; // 12% wins

    (f * precision).round() / precision
}
