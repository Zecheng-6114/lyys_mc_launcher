use eframe::egui;

const CONTOUR_SEGMENTS: usize = 96;

/// One topographic "hill": a stack of nested, wobbly closed rings.
struct ContourHill {
    /// Center in normalized panel coordinates (0..=1).
    center: (f32, f32),
    /// Radius of the innermost ring, as a fraction of the panel's short side.
    inner: f32,
    /// Distance between neighbouring rings, same unit as `inner`.
    spacing: f32,
    rings: usize,
    /// Radius modulation amplitudes at 2x, 3x and 5x the angle.
    wobble: (f32, f32, f32),
    phase: f32,
    /// Horizontal stretch before rotation.
    stretch: f32,
    rotation: f32,
}

const CONTOUR_HILLS: [ContourHill; 3] = [
    ContourHill {
        center: (0.06, 0.94),
        inner: 0.05,
        spacing: 0.055,
        rings: 7,
        wobble: (0.10, 0.05, 0.03),
        phase: 0.4,
        stretch: 1.25,
        rotation: 0.35,
    },
    ContourHill {
        center: (0.92, 0.06),
        inner: 0.04,
        spacing: 0.045,
        rings: 6,
        wobble: (0.09, 0.06, 0.02),
        phase: 2.1,
        stretch: 1.15,
        rotation: -0.5,
    },
    ContourHill {
        center: (0.62, 0.98),
        inner: 0.03,
        spacing: 0.05,
        rings: 5,
        wobble: (0.12, 0.04, 0.03),
        phase: 4.0,
        stretch: 1.1,
        rotation: 0.8,
    },
];

/// Paint faint contour rings across the given rect.
pub(crate) fn paint_contour_background(
    painter: &egui::Painter,
    rect: egui::Rect,
    visuals: &egui::Visuals,
) {
    let stroke = egui::Stroke::new(1.0, visuals.weak_text_color().gamma_multiply(0.35));
    let short = rect.width().min(rect.height());
    let mut shapes = Vec::new();
    for hill in &CONTOUR_HILLS {
        let center = rect.lerp_inside(egui::vec2(hill.center.0, hill.center.1));
        let (sin_rot, cos_rot) = hill.rotation.sin_cos();
        for ring in 0..hill.rings {
            let radius = (hill.inner + hill.spacing * ring as f32) * short;
            let mut points = Vec::with_capacity(CONTOUR_SEGMENTS);
            for step in 0..CONTOUR_SEGMENTS {
                let angle = step as f32 / CONTOUR_SEGMENTS as f32 * std::f32::consts::TAU;
                let (sin_a, cos_a) = angle.sin_cos();
                let wobble = 1.0
                    + hill.wobble.0 * (2.0 * angle + hill.phase).sin()
                    + hill.wobble.1 * (3.0 * angle + hill.phase * 0.7).sin()
                    + hill.wobble.2 * (5.0 * angle + hill.phase * 1.9).sin();
                let x = cos_a * radius * wobble * hill.stretch;
                let y = sin_a * radius * wobble;
                points.push(
                    center + egui::vec2(x * cos_rot - y * sin_rot, x * sin_rot + y * cos_rot),
                );
            }
            shapes.push(egui::Shape::closed_line(points, stroke));
        }
    }
    painter.extend(shapes);
}
