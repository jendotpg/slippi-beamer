pub const WIDTH: u16 = 64;
pub const HEIGHT: u16 = 64;
pub const PIXELS: usize = WIDTH as usize * HEIGHT as usize;
pub const FRAME_BYTES: usize = PIXELS * 2;

const BLOB: &[u8] = include_bytes!("boot_animation.bin");

pub const FRAME_COUNT: usize = BLOB.len() / FRAME_BYTES;

const _: () = assert!(
    BLOB.len().is_multiple_of(FRAME_BYTES),
    "boot_animation.bin is not a whole number of 64x64 frames - re-run tools/encode_boot_animation.py",
);
const _: () = assert!(
    FRAME_COUNT >= 2,
    "the boot animation needs at least two frames"
);

pub fn frame(index: usize) -> &'static [u8] {
    let start = (index % FRAME_COUNT) * FRAME_BYTES;
    &BLOB[start..start + FRAME_BYTES]
}
