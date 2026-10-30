//! Geometry and colours of nodes: where each port sits, how big a node is.
//! Shared by drawing and hit-testing so they can never disagree.

use eframe::egui::{Color32, Pos2, Rect, Vec2};
use helmsman_graph::{Kind, Node, Port, Project, Ty};

pub const HEADER: f32 = 24.0;
pub const ROW: f32 = 22.0;
pub const PORT_R: f32 = 6.0;

pub const BG: Color32 = Color32::from_rgb(22, 24, 29);
pub const GRID: Color32 = Color32::from_rgb(40, 44, 52);
pub const BODY: Color32 = Color32::from_rgb(42, 46, 56);
pub const TEXT: Color32 = Color32::from_rgb(222, 226, 235);
pub const DIM: Color32 = Color32::from_rgb(130, 138, 152);
pub const SELECT: Color32 = Color32::from_rgb(255, 200, 60);
pub const ON: Color32 = Color32::from_rgb(80, 230, 120);
pub const OFF: Color32 = Color32::from_rgb(70, 76, 88);
pub const NUM: Color32 = Color32::from_rgb(90, 160, 255);
pub const ERR: Color32 = Color32::from_rgb(255, 110, 100);

pub struct Layout {
    pub rect: Rect,
    pub header: Rect,
    /// The switch, lamp, readout, progress bar or preview under the ports.
    pub widget: Option<Rect>,
    pub ins: Vec<(Port, Pos2)>,
    pub outs: Vec<(Port, Pos2)>,
}

pub fn layout(project: &Project, node: &Node) -> Layout {
    let (ins, outs) = project.ports(&node.kind);
    let w = match node.kind {
        Kind::Block(_) => 190.0,
        Kind::Note { .. } => 210.0,
        Kind::PinIn { .. } | Kind::PinOut { .. } => 120.0,
        _ => 160.0,
    };
    let rows = ins.len().max(outs.len()) as f32;
    let extra = match &node.kind {
        Kind::Toggle { .. }
        | Kind::Button
        | Kind::Lamp
        | Kind::Display
        | Kind::Const { .. }
        | Kind::Clock { .. } => 34.0,
        Kind::Ton { .. } | Kind::Tof { .. } | Kind::Tp { .. } | Kind::Tonr { .. } | Kind::Ctu { .. } => 18.0,
        Kind::Block(_) => 56.0,
        Kind::Note { text } => 16.0 * text.lines().count().max(1) as f32 + 10.0,
        _ => 0.0,
    };
    let min = Pos2::new(node.pos[0], node.pos[1]);
    let rect = Rect::from_min_size(min, Vec2::new(w, HEADER + rows * ROW + extra + 6.0));
    let header = Rect::from_min_size(min, Vec2::new(w, HEADER));
    let row_y = |i: usize| min.y + HEADER + ROW * (i as f32 + 0.5) + 2.0;
    let widget = (extra > 0.0).then(|| {
        Rect::from_min_max(
            Pos2::new(min.x + 8.0, min.y + HEADER + rows * ROW + 3.0),
            Pos2::new(min.x + w - 8.0, rect.max.y - 6.0),
        )
    });
    Layout {
        rect,
        header,
        widget,
        ins: ins
            .into_iter()
            .enumerate()
            .map(|(i, p)| (p, Pos2::new(min.x, row_y(i))))
            .collect(),
        outs: outs
            .into_iter()
            .enumerate()
            .map(|(i, p)| (p, Pos2::new(min.x + w, row_y(i))))
            .collect(),
    }
}

/// Header colour by what the node is.
pub fn accent(kind: &Kind) -> Color32 {
    match kind {
        Kind::Toggle { .. } | Kind::Button | Kind::Const { .. } | Kind::Clock { .. } => {
            Color32::from_rgb(38, 140, 130)
        }
        Kind::Lamp | Kind::Display => Color32::from_rgb(196, 140, 40),
        Kind::And { .. } | Kind::Or { .. } | Kind::Xor | Kind::Not => Color32::from_rgb(70, 100, 170),
        Kind::Sr | Kind::Rs | Kind::RTrig | Kind::FTrig => Color32::from_rgb(130, 80, 180),
        Kind::Ton { .. } | Kind::Tof { .. } | Kind::Tp { .. } | Kind::Tonr { .. } => {
            Color32::from_rgb(200, 100, 50)
        }
        Kind::Ctu { .. } => Color32::from_rgb(190, 70, 120),
        Kind::Math(_) | Kind::Compare(_) | Kind::Select => Color32::from_rgb(70, 130, 90),
        Kind::PinIn { .. } | Kind::PinOut { .. } => Color32::from_rgb(40, 150, 190),
        Kind::Block(_) => Color32::from_rgb(150, 50, 140),
        Kind::Note { .. } => Color32::from_rgb(150, 140, 70),
    }
}

pub fn wire_color(ty: Ty, live: Option<bool>) -> Color32 {
    match (ty, live) {
        (Ty::Bool, Some(true)) => ON,
        (Ty::Bool, _) => Color32::from_rgb(120, 128, 140),
        (Ty::Num, _) => NUM,
    }
}

/// Short text for a number: integers without a fraction, others to 2 places.
pub fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e12 {
        format!("{n:.0}")
    } else {
        format!("{n:.2}")
    }
}
