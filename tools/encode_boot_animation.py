#!/usr/bin/env python3
"""Turn assets/boot_animation/frameNN.png into src/status/boot_animation.bin as 
big-endian RGB565 frames.
"""

import sys
from pathlib import Path

import click

try:
    from PIL import Image
except ImportError:
    sys.exit("this needs Pillow: pip install pillow")

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "assets" / "boot_animation"
OUTPUT = ROOT / "src" / "status" / "boot_animation.bin"

SIDE = 64
PIXELS = SIDE * SIDE
FRAME_BYTES = PIXELS * 2

PREFIX = "frame"
SUFFIX = "png"
DIGITS = "0123456789"
HIGHEST_FRAME = 16


def frame_number(path):
    parts = path.name.split(".")
    if len(parts) != 2 or parts[1] != SUFFIX:
        return None

    basename = parts[0]
    if not basename.startswith(PREFIX):
        return None

    digits = basename[-2:]
    if len(digits) != 2 or digits[0] not in DIGITS or digits[1] not in DIGITS:
        raise click.ClickException(
            f"{path.name} starts with '{PREFIX}' but does not end in two digits; "
            f"frames are named {PREFIX}00.{SUFFIX} upwards"
        )

    number = int(digits)
    if number > HIGHEST_FRAME:
        raise click.ClickException(
            f"{path.name} is numbered {number}; frames run from 0 to {HIGHEST_FRAME}"
        )
    return number


def load_frames():
    found = {}
    for path in sorted(ASSETS.iterdir()):
        number = frame_number(path)
        if number is not None:
            found[number] = path

    if not found:
        raise click.ClickException(f"no {PREFIX}NN.{SUFFIX} in {ASSETS}")
    expected = list(range(len(found)))
    if sorted(found) != expected:
        missing = [f"{PREFIX}{n:02d}.{SUFFIX}" for n in expected if n not in found]
        raise click.ClickException(
            f"the frames must be numbered {PREFIX}00.{SUFFIX} upwards with no gaps; "
            f"missing {missing}"
        )
    if len(found) < 2:
        raise click.ClickException("an animation needs at least two frames")

    frames = []
    for number in expected:
        path = found[number]
        image = Image.open(path).convert("RGBA")
        if image.size != (SIDE, SIDE):
            raise click.ClickException(
                f"{path.name} is {image.size[0]}x{image.size[1]}; "
                f"every frame must be exactly {SIDE}x{SIDE}"
            )
        raw = image.tobytes()
        frames.append([rgb565(*raw[i:i + 4]) for i in range(0, len(raw), 4)])
    return frames


def rgb565(r, g, b, a):
    if a == 0:
        return 0  # the boot screen behind the sprite is black
    return ((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3)


def pack(frames):
    blob = bytearray()
    for frame in frames:
        for colour in frame:
            blob.append((colour >> 8) & 0xFF)
            blob.append(colour & 0xFF)

    click.echo(
        f"{len(frames)} frames of {SIDE}x{SIDE}, {len(blob):,} B "
        f"({FRAME_BYTES:,} B each)"
    )
    return bytes(blob)


def unpack(blob):
    frames = []
    for start in range(0, len(blob), FRAME_BYTES):
        chunk = blob[start:start + FRAME_BYTES]
        frames.append([
            chunk[i] << 8 | chunk[i + 1] for i in range(0, len(chunk), 2)
        ])
    return frames


def verify(frames, blob):
    decoded = unpack(blob)
    if len(decoded) != len(frames):
        raise click.ClickException(
            f"the blob did not read back: {len(decoded)} frames, "
            f"expected {len(frames)}"
        )
    for number, expected in enumerate(frames):
        got = decoded[number]
        if got != expected:
            bad = next(i for i, (a, b) in enumerate(zip(got, expected)) if a != b)
            raise click.ClickException(
                f"{PREFIX}{number:02d} does not round-trip: pixel {bad} "
                f"({bad % SIDE},{bad // SIDE}) is 0x{got[bad]:04X}, "
                f"should be 0x{expected[bad]:04X}"
            )
    click.echo(f"verified: all {len(frames)} frames round-trip pixel-exact")


def write_preview(frames, path, ms_per_frame):
    images = []
    for frame in frames:
        image = Image.new("RGB", (SIDE, SIDE))
        image.putdata([
            (((px >> 11) & 0x1F) << 3, ((px >> 5) & 0x3F) << 2, (px & 0x1F) << 3)
            for px in frame
        ])
        images.append(image)
    images[0].save(
        path, save_all=True, append_images=images[1:],
        duration=ms_per_frame, loop=0,
    )
    click.echo(f"wrote {path}")


@click.command(
    context_settings={"help_option_names": ["-h", "--help"]},
    help=__doc__,
)
@click.option(
    "--verify", "run_verify", is_flag=True,
    help="Decode the generated tables back and check every frame against its PNG.",
)
@click.option(
    "--preview", "preview_path", metavar="PATH",
    type=click.Path(dir_okay=False, path_type=Path),
    help="Write the decoded loop to PATH as an APNG, to see it without a beamer.",
)
@click.option(
    "--ms-per-frame", default=100, show_default=True, metavar="MS",
    help="Frame duration for --preview. Match the firmware's BOOT_MS_PER_FRAME.",
)
def main(run_verify, preview_path, ms_per_frame):
    frames = load_frames()
    blob = pack(frames)

    OUTPUT.write_bytes(blob)
    click.echo(f"wrote {OUTPUT.relative_to(ROOT)}")

    if run_verify:
        verify(frames, blob)
    if preview_path:
        write_preview(unpack(blob), preview_path, ms_per_frame)


if __name__ == "__main__":
    main()
