use egui::epaint::{self, Color32, Pos2, Rect, Stroke, TextureId};

/// Like [`epaint::Mesh`], but optimized for transport over a network.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct NetMesh {
    pub texture_id: TextureId,
    pub indices: Vec<u32>,
    pub pos: Vec<Pos2>,
    pub uv: Vec<Pos2>,
    pub color: Vec<Color32>,
}

impl From<&epaint::Mesh> for NetMesh {
    fn from(mesh: &epaint::Mesh) -> Self {
        Self {
            texture_id: mesh.texture_id.clone(),
            indices: mesh.indices.clone(),
            pos: mesh.vertices.iter().map(|v| v.pos).collect(),
            uv: mesh.vertices.iter().map(|v| v.uv).collect(),
            color: mesh.vertices.iter().map(|v| v.color).collect(),
        }
    }
}

impl From<&NetMesh> for epaint::Mesh {
    fn from(mesh: &NetMesh) -> epaint::Mesh {
        epaint::Mesh {
            texture_id: mesh.texture_id.clone(),
            indices: mesh.indices.clone(),
            vertices: itertools::izip!(&mesh.pos, &mesh.uv, &mesh.color)
                .map(|(&pos, &uv, &color)| epaint::Vertex { pos, uv, color })
                .collect(),
        }
    }
}

// ----------------------------------------------------------------------------

/// Like [`epaint::Shape`], but optimized for transport over a network.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub enum NetShape {
    Circle(epaint::CircleShape),
    LineSegment { points: [Pos2; 2], stroke: Stroke },
    Path(epaint::PathShape),
    Rect(epaint::RectShape),
    Text(NetTextShape),
    Mesh(NetMesh),
}

/// How to draw some text on screen.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct NetTextShape {
    pub pos: Pos2,
    pub job: epaint::text::LayoutJob,
    pub underline: Stroke,
    pub override_text_color: Option<Color32>,
    pub angle: f32,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ClippedNetShape(Rect, NetShape);

pub fn to_clipped_net_shapes(in_shapes: Vec<epaint::ClippedShape>) -> Vec<ClippedNetShape> {
    let mut net_shapes = vec![];
    for epaint::ClippedShape(clip_rect, shape) in in_shapes {
        to_net_shapes(clip_rect, shape, &mut net_shapes)
    }
    net_shapes
}

fn to_net_shapes(
    clip_rect: Rect,
    in_shape: epaint::Shape,
    out_net_shapes: &mut Vec<ClippedNetShape>,
) {
    // TODO: cull here!
    match in_shape {
        epaint::Shape::Noop => {}
        epaint::Shape::Vec(shapes) => {
            for shape in shapes {
                to_net_shapes(clip_rect, shape, out_net_shapes);
            }
        }
        epaint::Shape::Circle(circle_shape) => {
            out_net_shapes.push(ClippedNetShape(clip_rect, NetShape::Circle(circle_shape)));
        }
        epaint::Shape::LineSegment { points, stroke } => {
            out_net_shapes.push(ClippedNetShape(
                clip_rect,
                NetShape::LineSegment { points, stroke },
            ));
        }
        epaint::Shape::Path(path_shape) => {
            out_net_shapes.push(ClippedNetShape(clip_rect, NetShape::Path(path_shape)));
        }
        epaint::Shape::Rect(rect_shape) => {
            out_net_shapes.push(ClippedNetShape(clip_rect, NetShape::Rect(rect_shape)));
        }
        epaint::Shape::Text(text_shape) => {
            out_net_shapes.push(ClippedNetShape(
                clip_rect,
                NetShape::Text(NetTextShape {
                    pos: text_shape.pos,
                    job: (*text_shape.galley.job).clone(),
                    underline: text_shape.underline,
                    override_text_color: text_shape.override_text_color,
                    angle: text_shape.angle,
                }),
            ));
        }
        epaint::Shape::Mesh(mesh) => {
            out_net_shapes.push(ClippedNetShape(
                clip_rect,
                NetShape::Mesh(NetMesh::from(&mesh)),
            ));
        }
    }
}

pub fn from_clipped_net_shapes(
    fonts: &epaint::text::Fonts,
    in_shapes: Vec<ClippedNetShape>,
) -> Vec<epaint::ClippedShape> {
    in_shapes
        .into_iter()
        .map(|ClippedNetShape(clip_rect, net_shape)| {
            epaint::ClippedShape(clip_rect, to_epaint_shape(fonts, net_shape))
        })
        .collect()
}

fn to_epaint_shape(fonts: &epaint::text::Fonts, net_shape: NetShape) -> epaint::Shape {
    match net_shape {
        NetShape::Circle(circle_shape) => epaint::Shape::Circle(circle_shape),
        NetShape::LineSegment { points, stroke } => epaint::Shape::LineSegment { points, stroke },
        NetShape::Path(path_shape) => epaint::Shape::Path(path_shape),
        NetShape::Rect(rect_shape) => epaint::Shape::Rect(rect_shape),
        NetShape::Text(text_shape) => {
            let galley = fonts.layout_job(text_shape.job);
            epaint::Shape::Text(epaint::TextShape {
                pos: text_shape.pos,
                galley,
                underline: text_shape.underline,
                override_text_color: text_shape.override_text_color,
                angle: text_shape.angle,
            })
        }
        NetShape::Mesh(net_mesh) => epaint::Shape::Mesh(epaint::Mesh::from(&net_mesh)),
    }
}
