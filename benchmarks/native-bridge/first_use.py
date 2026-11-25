"""Generate separately compiled leaf functions and a reference digest for each traversal."""
from pathlib import Path

BANKS = 4
LEAVES = 1024
SEED = 0x34199A4DBE31F47A
MASK = (1 << 64) - 1


def constant(leaf):
    return (leaf * 2654435761 + 2246822519) & 0xffffffff


def golden():
    digests = []
    for bank in range(BANKS):
        value = SEED ^ bank
        for step in range(LEAVES):
            leaf = bank * LEAVES + (step * 421 + 113) % LEAVES
            rotated = ((value << 7) & MASK) | (value >> 57)
            value = ((rotated ^ constant(leaf)) * (2 * leaf + 3) + 17) & MASK
        digests.append(f"{value:016x}")
    return {"banks": BANKS, "leaves_per_bank": LEAVES, "bank_digests": digests}


def generate(directory: Path):
    names = [f"leaf_{idx:04d}" for idx in range(BANKS * LEAVES)]
    with (directory / "first_use.inc").open("w", encoding="ascii", newline="\n") as out:
        out.write(f"#define BANKS {BANKS}\n#define LEAVES {LEAVES}\n")
        for leaf, name in enumerate(names):
            out.write(f"static __attribute__((noinline)) uint64_t {name}(uint64_t v) {{\n"
                      f"    return (((v << 7 | v >> 57) ^ UINT64_C({constant(leaf)})) "
                      f"* UINT64_C({2 * leaf + 3})) + 17;\n}}\n")
        out.write("static uint64_t (*const leaves[])(uint64_t) = {\n")
        out.write(",\n".join(names) + "\n};\n")
