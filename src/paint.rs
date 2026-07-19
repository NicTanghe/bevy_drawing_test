use bevy::{
    asset::RenderAssetUsages,
    image::ImageSampler,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};

pub const CANVAS_WIDTH: u32 = 1_600;
pub const CANVAS_HEIGHT: u32 = 1_000;
pub const PAPER: [u8; 4] = [248, 247, 244, 255];
const INK: [u8; 4] = [24, 27, 32, 255];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Pen,
    Eraser,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pen => "PEN",
            Self::Eraser => "ERASER",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BrushSample {
    /// Position in logical window pixels, with the origin at the top left.
    pub position: Vec2,
    /// `None` means the device has no pressure axis (the mouse fallback uses this).
    pub pressure: Option<f32>,
    /// Tilt in degrees on the screen's x/y axes.
    pub tilt: Vec2,
    pub tool: Tool,
}

impl BrushSample {
    fn interpolate(self, other: Self, amount: f32) -> Self {
        let pressure = match (self.pressure, other.pressure) {
            (Some(a), Some(b)) => Some(a.lerp(b, amount)),
            (_, pressure) => pressure.or(self.pressure),
        };

        Self {
            position: self.position.lerp(other.position, amount),
            pressure,
            tilt: self.tilt.lerp(other.tilt, amount),
            tool: other.tool,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BrushShape {
    /// Half-size in logical window pixels. X is the tilt-facing major axis.
    pub half_size: Vec2,
    /// Clockwise angle in screen coordinates, where y points down.
    pub angle: f32,
    pub opacity: f32,
}

impl BrushShape {
    pub fn from_sample(sample: BrushSample, nominal_diameter: f32) -> Self {
        let pressure = sample.pressure.unwrap_or(1.0).clamp(0.0, 1.0);
        let pressure_size = 0.22 + 0.78 * pressure.sqrt();
        let tilt_amount = (sample.tilt.length() / 75.0).clamp(0.0, 1.0);
        let radius = nominal_diameter * 0.5 * pressure_size;

        // An upright pen is round. Tilting stretches the footprint toward the
        // tilt vector while slightly pinching its cross-axis.
        let major = radius * (1.0 + 1.55 * tilt_amount);
        let minor = radius * (1.0 - 0.35 * tilt_amount);
        let angle = if sample.tilt.length_squared() > 0.01 {
            sample.tilt.y.atan2(sample.tilt.x)
        } else {
            0.0
        };
        let opacity = if sample.pressure.is_some() {
            0.12 + 0.88 * pressure.powf(0.7)
        } else {
            1.0
        };

        Self {
            half_size: Vec2::new(major.max(0.65), minor.max(0.65)),
            angle,
            opacity,
        }
    }
}

#[derive(Resource)]
pub struct PaintCanvas {
    pub image: Handle<Image>,
    size: UVec2,
    background: [u8; 4],
}

impl PaintCanvas {
    pub fn new(images: &mut Assets<Image>) -> Self {
        let mut image = Image::new_fill(
            Extent3d {
                width: CANVAS_WIDTH,
                height: CANVAS_HEIGHT,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &PAPER,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        );
        image.sampler = ImageSampler::linear();

        Self {
            image: images.add(image),
            size: UVec2::new(CANVAS_WIDTH, CANVAS_HEIGHT),
            background: PAPER,
        }
    }

    pub fn apply(&self, image: &mut Image, operation: PaintOperation) {
        match operation {
            PaintOperation::Clear => self.clear(image),
            PaintOperation::Stroke {
                from,
                to,
                nominal_diameter,
                viewport_size,
            } => self.paint_segment(image, from, to, nominal_diameter, viewport_size),
        }
    }

    fn clear(&self, image: &mut Image) {
        let Some(data) = image.data.as_mut() else {
            return;
        };
        for pixel in data.chunks_exact_mut(4) {
            pixel.copy_from_slice(&self.background);
        }
    }

    fn paint_segment(
        &self,
        image: &mut Image,
        from: Option<BrushSample>,
        to: BrushSample,
        nominal_diameter: f32,
        viewport_size: Vec2,
    ) {
        let Some(from) = from.filter(|from| from.tool == to.tool) else {
            self.stamp(image, to, nominal_diameter, viewport_size);
            return;
        };

        let distance = from.position.distance(to.position);
        if distance <= f32::EPSILON {
            self.stamp(image, to, nominal_diameter, viewport_size);
            return;
        }

        let from_shape = BrushShape::from_sample(from, nominal_diameter);
        let to_shape = BrushShape::from_sample(to, nominal_diameter);
        let spacing = from_shape
            .half_size
            .min_element()
            .min(to_shape.half_size.min_element())
            .mul_add(0.32, 0.0)
            .max(0.7);
        let steps = (distance / spacing).ceil().max(1.0) as usize;

        for step in 1..=steps {
            let amount = step as f32 / steps as f32;
            self.stamp(
                image,
                from.interpolate(to, amount),
                nominal_diameter,
                viewport_size,
            );
        }
    }

    fn stamp(
        &self,
        image: &mut Image,
        sample: BrushSample,
        nominal_diameter: f32,
        viewport_size: Vec2,
    ) {
        if viewport_size.x <= 0.0 || viewport_size.y <= 0.0 {
            return;
        }
        let Some(data) = image.data.as_mut() else {
            return;
        };

        let shape = BrushShape::from_sample(sample, nominal_diameter);
        let scale = self.size.as_vec2() / viewport_size;
        let center = sample.position * scale;
        let (sin, cos) = shape.angle.sin_cos();

        // Transform the rotated ellipse's screen-space bounds into texture pixels.
        let extent_x = scale.x * (cos.abs() * shape.half_size.x + sin.abs() * shape.half_size.y);
        let extent_y = scale.y * (sin.abs() * shape.half_size.x + cos.abs() * shape.half_size.y);
        let min_x = (center.x - extent_x - 1.0).floor().max(0.0) as u32;
        let max_x = (center.x + extent_x + 1.0)
            .ceil()
            .min(self.size.x as f32 - 1.0) as u32;
        let min_y = (center.y - extent_y - 1.0).floor().max(0.0) as u32;
        let max_y = (center.y + extent_y + 1.0)
            .ceil()
            .min(self.size.y as f32 - 1.0) as u32;
        let target = match sample.tool {
            Tool::Pen => INK,
            Tool::Eraser => self.background,
        };

        for y in min_y..=max_y {
            for x in min_x..=max_x {
                // Evaluate the ellipse in logical screen pixels so brush geometry
                // stays correct even when the window and texture have different aspects.
                let offset = (Vec2::new(x as f32 + 0.5, y as f32 + 0.5) - center) / scale;
                let local = Vec2::new(
                    cos * offset.x + sin * offset.y,
                    -sin * offset.x + cos * offset.y,
                );
                let normalized = (local / shape.half_size).length();
                let coverage =
                    ((1.0 - normalized) * shape.half_size.min_element() + 0.5).clamp(0.0, 1.0);
                if coverage <= 0.0 {
                    continue;
                }

                let alpha = coverage * shape.opacity;
                let index = ((y * self.size.x + x) * 4) as usize;
                blend_pixel(&mut data[index..index + 4], target, alpha);
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum PaintOperation {
    Clear,
    Stroke {
        from: Option<BrushSample>,
        to: BrushSample,
        nominal_diameter: f32,
        viewport_size: Vec2,
    },
}

fn blend_pixel(pixel: &mut [u8], target: [u8; 4], alpha: f32) {
    let alpha = alpha.clamp(0.0, 1.0);
    for channel in 0..3 {
        let old = pixel[channel] as f32;
        let new = target[channel] as f32;
        pixel[channel] = old.lerp(new, alpha).round() as u8;
    }
    pixel[3] = u8::MAX;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_image() -> Image {
        Image::new_fill(
            Extent3d {
                width: CANVAS_WIDTH,
                height: CANVAS_HEIGHT,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &PAPER,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::MAIN_WORLD,
        )
    }

    fn test_canvas() -> PaintCanvas {
        PaintCanvas {
            image: Handle::default(),
            size: UVec2::new(CANVAS_WIDTH, CANVAS_HEIGHT),
            background: PAPER,
        }
    }

    #[test]
    fn pressure_changes_size_and_opacity() {
        let base = BrushSample {
            position: Vec2::ZERO,
            pressure: Some(0.1),
            tilt: Vec2::ZERO,
            tool: Tool::Pen,
        };
        let light = BrushShape::from_sample(base, 20.0);
        let firm = BrushShape::from_sample(
            BrushSample {
                pressure: Some(1.0),
                ..base
            },
            20.0,
        );

        assert!(firm.half_size.x > light.half_size.x);
        assert!(firm.opacity > light.opacity);
    }

    #[test]
    fn tilt_stretches_in_its_direction() {
        let shape = BrushShape::from_sample(
            BrushSample {
                position: Vec2::ZERO,
                pressure: Some(1.0),
                tilt: Vec2::new(0.0, 60.0),
                tool: Tool::Pen,
            },
            20.0,
        );

        assert!(shape.half_size.x > shape.half_size.y);
        assert!((shape.angle - std::f32::consts::FRAC_PI_2).abs() < 0.001);
    }

    #[test]
    fn eraser_restores_the_paper_color() {
        let canvas = test_canvas();
        let mut image = test_image();
        let position = Vec2::new(800.0, 500.0);
        let viewport = Vec2::new(CANVAS_WIDTH as f32, CANVAS_HEIGHT as f32);
        let sample = BrushSample {
            position,
            pressure: None,
            tilt: Vec2::ZERO,
            tool: Tool::Pen,
        };

        canvas.stamp(&mut image, sample, 20.0, viewport);
        let center = ((500 * CANVAS_WIDTH + 800) * 4) as usize;
        assert_ne!(&image.data.as_ref().unwrap()[center..center + 4], &PAPER);

        canvas.stamp(
            &mut image,
            BrushSample {
                tool: Tool::Eraser,
                ..sample
            },
            24.0,
            viewport,
        );
        assert_eq!(&image.data.as_ref().unwrap()[center..center + 4], &PAPER);
    }
}
