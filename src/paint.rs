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
const TILE_SIZE: u32 = 256;

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
    tiles: Vec<CanvasTile>,
    pixels: Vec<u8>,
    size: UVec2,
    background: [u8; 4],
    dirty_tiles: Vec<bool>,
}

pub struct CanvasTile {
    pub image: Handle<Image>,
    pub origin: UVec2,
    pub size: UVec2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PixelRect {
    /// Inclusive top-left pixel.
    min: UVec2,
    /// Exclusive bottom-right pixel.
    max: UVec2,
}

impl PixelRect {
    fn intersects(self, other: Self) -> bool {
        self.min.x < other.max.x
            && self.max.x > other.min.x
            && self.min.y < other.max.y
            && self.max.y > other.min.y
    }
}

impl PaintCanvas {
    pub fn new(images: &mut Assets<Image>) -> Self {
        let size = UVec2::new(CANVAS_WIDTH, CANVAS_HEIGHT);
        let mut tiles = Vec::new();
        for y in (0..size.y).step_by(TILE_SIZE as usize) {
            for x in (0..size.x).step_by(TILE_SIZE as usize) {
                let tile_size = UVec2::new(TILE_SIZE.min(size.x - x), TILE_SIZE.min(size.y - y));
                let mut image = Image::new_fill(
                    Extent3d {
                        width: tile_size.x,
                        height: tile_size.y,
                        depth_or_array_layers: 1,
                    },
                    TextureDimension::D2,
                    &PAPER,
                    TextureFormat::Rgba8UnormSrgb,
                    RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
                );
                image.sampler = ImageSampler::linear();
                tiles.push(CanvasTile {
                    image: images.add(image),
                    origin: UVec2::new(x, y),
                    size: tile_size,
                });
            }
        }

        let tile_count = tiles.len();
        Self {
            tiles,
            pixels: PAPER.repeat((size.x * size.y) as usize),
            size,
            background: PAPER,
            dirty_tiles: vec![false; tile_count],
        }
    }

    pub fn tiles(&self) -> impl Iterator<Item = &CanvasTile> {
        self.tiles.iter()
    }

    pub fn size(&self) -> UVec2 {
        self.size
    }

    pub fn apply(&mut self, operation: PaintOperation) {
        match operation {
            PaintOperation::Clear => self.clear(),
            PaintOperation::Stroke {
                from,
                to,
                nominal_diameter,
                viewport_size,
            } => self.paint_segment(from, to, nominal_diameter, viewport_size),
        }
    }

    /// Copies only tiles touched since the previous upload into Bevy's image assets.
    /// The renderer then uploads those small textures instead of the full canvas.
    pub fn upload_dirty(&mut self, images: &mut Assets<Image>) -> usize {
        let mut uploaded_bytes = 0;

        for (tile_index, tile) in self.tiles.iter().enumerate() {
            if !self.dirty_tiles[tile_index] {
                continue;
            }
            let Some(mut image) = images.get_mut(&tile.image) else {
                continue;
            };
            let byte_len = (tile.size.x * tile.size.y * 4) as usize;
            let data = image.data.get_or_insert_with(|| vec![0; byte_len]);
            if data.len() != byte_len {
                data.resize(byte_len, 0);
            }

            let source_stride = self.size.x as usize * 4;
            let tile_stride = tile.size.x as usize * 4;
            for row in 0..tile.size.y as usize {
                let source_start =
                    (tile.origin.y as usize + row) * source_stride + tile.origin.x as usize * 4;
                let destination_start = row * tile_stride;
                data[destination_start..destination_start + tile_stride]
                    .copy_from_slice(&self.pixels[source_start..source_start + tile_stride]);
            }
            self.dirty_tiles[tile_index] = false;
            uploaded_bytes += byte_len;
        }

        uploaded_bytes
    }

    fn clear(&mut self) {
        for pixel in self.pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&self.background);
        }
        self.dirty_tiles.fill(true);
    }

    fn paint_segment(
        &mut self,
        from: Option<BrushSample>,
        to: BrushSample,
        nominal_diameter: f32,
        viewport_size: Vec2,
    ) {
        let Some(from) = from.filter(|from| from.tool == to.tool) else {
            self.stamp(to, nominal_diameter, viewport_size);
            return;
        };

        let distance = from.position.distance(to.position);
        if distance <= f32::EPSILON {
            self.stamp(to, nominal_diameter, viewport_size);
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
                from.interpolate(to, amount),
                nominal_diameter,
                viewport_size,
            );
        }
    }

    fn stamp(&mut self, sample: BrushSample, nominal_diameter: f32, viewport_size: Vec2) {
        if viewport_size.x <= 0.0 || viewport_size.y <= 0.0 {
            return;
        }

        let shape = BrushShape::from_sample(sample, nominal_diameter);
        let scale = self.size.as_vec2() / viewport_size;
        let center = sample.position * scale;
        let (sin, cos) = shape.angle.sin_cos();

        // Transform the rotated ellipse's screen-space bounds into texture pixels.
        let extent_x = scale.x * (cos.abs() * shape.half_size.x + sin.abs() * shape.half_size.y);
        let extent_y = scale.y * (sin.abs() * shape.half_size.x + cos.abs() * shape.half_size.y);
        let raw_min = center - Vec2::new(extent_x, extent_y) - Vec2::ONE;
        let raw_max = center + Vec2::new(extent_x, extent_y) + Vec2::ONE;
        if raw_max.x < 0.0
            || raw_max.y < 0.0
            || raw_min.x >= self.size.x as f32
            || raw_min.y >= self.size.y as f32
        {
            return;
        }
        let min_x = raw_min.x.floor().max(0.0) as u32;
        let max_x = raw_max.x.ceil().min(self.size.x as f32 - 1.0) as u32;
        let min_y = raw_min.y.floor().max(0.0) as u32;
        let max_y = raw_max.y.ceil().min(self.size.y as f32 - 1.0) as u32;
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
                blend_pixel(&mut self.pixels[index..index + 4], target, alpha);
            }
        }
        self.mark_dirty(PixelRect {
            min: UVec2::new(min_x, min_y),
            max: UVec2::new(max_x + 1, max_y + 1),
        });
    }

    fn mark_dirty(&mut self, rect: PixelRect) {
        for (tile_index, tile) in self.tiles.iter().enumerate() {
            let tile_rect = PixelRect {
                min: tile.origin,
                max: tile.origin + tile.size,
            };
            if rect.intersects(tile_rect) {
                self.dirty_tiles[tile_index] = true;
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

    fn test_canvas() -> PaintCanvas {
        PaintCanvas {
            tiles: Vec::new(),
            pixels: PAPER.repeat((CANVAS_WIDTH * CANVAS_HEIGHT) as usize),
            size: UVec2::new(CANVAS_WIDTH, CANVAS_HEIGHT),
            background: PAPER,
            dirty_tiles: Vec::new(),
        }
    }

    fn test_tiled_canvas() -> PaintCanvas {
        let mut canvas = test_canvas();
        for y in (0..canvas.size.y).step_by(TILE_SIZE as usize) {
            for x in (0..canvas.size.x).step_by(TILE_SIZE as usize) {
                let size = UVec2::new(
                    TILE_SIZE.min(canvas.size.x - x),
                    TILE_SIZE.min(canvas.size.y - y),
                );
                canvas.tiles.push(CanvasTile {
                    image: Handle::default(),
                    origin: UVec2::new(x, y),
                    size,
                });
            }
        }
        canvas.dirty_tiles = vec![false; canvas.tiles.len()];
        canvas
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
        let mut canvas = test_canvas();
        let position = Vec2::new(800.0, 500.0);
        let viewport = Vec2::new(CANVAS_WIDTH as f32, CANVAS_HEIGHT as f32);
        let sample = BrushSample {
            position,
            pressure: None,
            tilt: Vec2::ZERO,
            tool: Tool::Pen,
        };

        canvas.stamp(sample, 20.0, viewport);
        let center = ((500 * CANVAS_WIDTH + 800) * 4) as usize;
        assert_ne!(&canvas.pixels[center..center + 4], &PAPER);

        canvas.stamp(
            BrushSample {
                tool: Tool::Eraser,
                ..sample
            },
            24.0,
            viewport,
        );
        assert_eq!(&canvas.pixels[center..center + 4], &PAPER);
    }

    #[test]
    fn a_small_dab_marks_only_its_tile() {
        let mut canvas = test_tiled_canvas();
        canvas.stamp(
            BrushSample {
                position: Vec2::new(400.0, 400.0),
                pressure: None,
                tilt: Vec2::ZERO,
                tool: Tool::Pen,
            },
            18.0,
            Vec2::new(CANVAS_WIDTH as f32, CANVAS_HEIGHT as f32),
        );

        assert_eq!(canvas.dirty_tiles.iter().filter(|dirty| **dirty).count(), 1);
    }

    #[test]
    fn distant_dabs_do_not_dirty_tiles_between_them() {
        let mut canvas = test_tiled_canvas();
        let viewport = Vec2::new(CANVAS_WIDTH as f32, CANVAS_HEIGHT as f32);
        for position in [Vec2::new(100.0, 100.0), Vec2::new(1_500.0, 900.0)] {
            canvas.stamp(
                BrushSample {
                    position,
                    pressure: None,
                    tilt: Vec2::ZERO,
                    tool: Tool::Pen,
                },
                18.0,
                viewport,
            );
        }

        assert_eq!(canvas.dirty_tiles.iter().filter(|dirty| **dirty).count(), 2);
    }

    #[test]
    fn uploading_a_dab_copies_one_tile_and_then_goes_idle() {
        let mut images = Assets::<Image>::default();
        let mut canvas = PaintCanvas::new(&mut images);
        canvas.stamp(
            BrushSample {
                position: Vec2::new(400.0, 400.0),
                pressure: None,
                tilt: Vec2::ZERO,
                tool: Tool::Pen,
            },
            18.0,
            Vec2::new(CANVAS_WIDTH as f32, CANVAS_HEIGHT as f32),
        );

        assert_eq!(
            canvas.upload_dirty(&mut images),
            (TILE_SIZE * TILE_SIZE * 4) as usize
        );
        assert_eq!(canvas.upload_dirty(&mut images), 0);
    }
}
