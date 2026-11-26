"""Generate the Windows PC-backend device-name index from its locked source ZIP.

Use the OpenRGB source ZIP from the Prismestra component distribution (see
components/openrgb/distribution-assets.json). Generate with:

    python scripts/build_device_model_catalog.py OPENRGB_SOURCE.zip

Pass --check to verify that the checked-in JSON is current. No compilation,
hardware access, or source extraction is required.

Coverage is limited to enabled named detector registrations and the explicit
Gigabyte Fusion 2 motherboard tables. Generic detector names may be present.
VID:PID aliases cover HID and USB serial registrations whose IDs are numeric
literals or resolve to numeric definitions in that detector's source file.
Runtime-discovered names (such as ASUS motherboard names from DMI or QMK names
from settings) and other model tables are outside this extract. A match is a
source indication. Hardware revisions, driver access, and lighting modes need
separate verification. The OpenRGB source is GPL-2.0; see the linked source
and components/openrgb/licenses/OpenRGB-GPL-2.0.txt.
"""

import argparse
from collections import defaultdict
from fnmatch import fnmatchcase
import json
from pathlib import Path, PurePosixPath
import re
from zipfile import ZipFile

from component_archive import file_pin, json_object


ROOT = Path(__file__).resolve().parents[1]
COMPONENT = ROOT / "components/openrgb"
OUTPUT = ROOT / "docs/devices/openrgb.json"
CPP_STRING = r'"(?:[^"\\\n]|\\.)*"'
CPP_COMMENT_OR_STRING = re.compile(rf"{CPP_STRING}|'(?:[^'\\\n]|\\.)*'|//[^\n]*|/\*.*?\*/", re.S)
REGISTRATION = re.compile(
    rf"^\s*(REGISTER_[A-Z0-9_]+)\s*\(\s*({CPP_STRING}|\w+)\s*,"
)
STRING_DEFINE = re.compile(rf"^\s*#\s*define\s+(\w+)\s+({CPP_STRING})\s*$")
HEX_ID = r"0[xX][0-9a-fA-F]{1,4}"
ID_DEFINE = re.compile(rf"^\s*#\s*define\s+(\w+)\s+({HEX_ID})\s*$")
ID_ARGUMENTS = re.compile(r"\s*\w+\s*,\s*(\w+)\s*,\s*(\w+)\s*[,)]")
DIRECTIVE = re.compile(r"^\s*#\s*(ifdef|ifndef|if|elif|else|endif)\b\s*(.*)")
WINDOWS_CONDITIONS = {
    ("ifdef", "_WIN32"): True,
    ("ifdef", "__linux__"): False,
    ("ifdef", "ENABLE_UNTESTED_MYSTIC_LIGHT"): False,
    ("if", "!defined(_WIN32)"): False,
    ("if", "defined(_WIN32) || defined(__APPLE__)"): True,
}
NON_WINDOWS = ("_Linux", "_FreeBSD", "_MacOS")
BRAND_PREFIXES = (
    "ASRock", "ASUS", "Cooler Master", "Corsair", "EVGA", "GALAX", "Gigabyte",
    "HyperX", "Kingston", "Lian Li", "Logitech", "MSI", "NZXT", "Patriot",
    "Razer", "ROCCAT", "Sapphire", "SteelSeries", "Thermaltake", "Wooting",
)


def remove_comments(source):
    """Keep offsets and line numbers intact for source links."""
    return CPP_COMMENT_OR_STRING.sub(
        lambda match: re.sub(r"[^\n]", " ", match[0]) if match[0].startswith("/") else match[0],
        source,
    )


def windows_lines(source):
    states = []
    for line_number, line in enumerate(remove_comments(source).splitlines(), 1):
        condition = DIRECTIVE.match(line)
        if condition:
            directive, expression = condition.groups()
            if directive == "endif":
                states.pop()
            elif directive == "else":
                states[-1] = None if states[-1] is None else not states[-1]
            elif directive == "elif":
                states[-1] = None
            else:
                # Unknown conditional bodies are omitted on both branches.
                states.append(WINDOWS_CONDITIONS.get((directive, expression.strip())))
        elif all(state is True for state in states):
            yield line_number, line


def windows_controller(path, exclusions):
    name = PurePosixPath(path).name
    return (
        path.startswith("Controllers/")
        and name.endswith(".cpp")
        and ("_Windows" in name or not any(platform in name for platform in NON_WINDOWS))
        and not any(fnmatchcase(path, pattern) for pattern in exclusions)
    )


def patch_exclusions():
    lock = json_object((COMPONENT / "source-lock.json").read_bytes())
    patch = COMPONENT / lock["patch"]
    file_pin(patch, lock["patch_sha256"])
    changes = patch.read_text(encoding="utf-8")
    if "prismestra_pc_backend" not in lock["configuration"]:
        raise ValueError("Review catalog extraction for the new OpenRGB build configuration")
    if re.search(r"^diff --git a/Controllers/|^[+-]\s*#\s*define\s+REGISTER_", changes, re.M):
        raise ValueError("Review catalog extraction for patched OpenRGB detector definitions")
    exclusions = re.findall(r'^\+\s*SOURCES -= \$\$files\("(Controllers/[^"]+)"\)$', changes, re.M)
    if exclusions != ["Controllers/PhilipsHueController/*.cpp"]:
        raise ValueError("Review catalog extraction for changed OpenRGB controller exclusions")
    return lock, exclusions


def registration_details(macro):
    if macro.startswith("REGISTER_HID_") or macro == "REGISTER_WOOTING_DETECTOR":
        return "HID", None
    if macro == "REGISTER_USB_SERIAL_DETECTOR":
        return "USB Serial", None
    if macro.startswith("REGISTER_I2C_"):
        device_type = {"REGISTER_I2C_DRAM_DETECTOR": "RAM", "REGISTER_I2C_PCI_DETECTOR": "GPU"}.get(macro)
        return "SMBus", device_type
    return None, None


def device_alias(connection, macro, arguments, definitions):
    if connection not in ("HID", "USB Serial") or macro.endswith("_ONLY"):
        return None
    if "HID_API_BUS_BLUETOOTH" in arguments or macro == "REGISTER_WOOTING_DETECTOR":
        return None
    match = ID_ARGUMENTS.match(arguments)
    if not match:
        return None
    ids = tuple(definitions.get(token, token) for token in match.groups())
    if all(re.fullmatch(HEX_ID, value) for value in ids):
        return ":".join(f"{int(value, 16):04X}" for value in ids)
    return None


def detector_names(source, macros):
    lines = list(windows_lines(source))
    literals = {}
    definitions = {}
    for _, line in lines:
        define = STRING_DEFINE.match(line)
        if define:
            literals[define[1]] = define[2]
        define_id = ID_DEFINE.match(line)
        if define_id:
            definitions[define_id[1]] = define_id[2]
    for line_number, line in lines:
        match = REGISTRATION.match(line)
        if not match or match[1] not in macros:
            continue
        token = match[2]
        literal = token if token.startswith('"') else literals.get(token)
        if literal is None:
            raise ValueError(f"Unresolved detector label {token} at line {line_number}")
        connection, device_type = registration_details(match[1])
        alias = device_alias(connection, match[1], line[match.end():], definitions)
        yield line_number, json.loads(literal), connection, device_type, alias


def gigabyte_board_names(archive):
    usb = "Controllers/GigabyteRGBFusion2USBController/GigabyteFusion2USB_Devices.cpp"
    smbus = "Controllers/GigabyteRGBFusion2SMBusController/GigabyteRGBFusion2SMBusControllerDetect.cpp"
    usb_source = remove_comments(archive.read(usb).decode("utf-8"))
    usb_definitions, usb_list = usb_source.split("const gb_fusion2_device* gb_fusion2_device_list_data[]", 1)
    usb_ids = set(re.findall(r"&([a-z0-9_]+),", usb_list.split("};", 1)[0])) - {"null"}
    for match in re.finditer(rf"static const gb_fusion2_device (\w+)\s*=\s*\{{[^{{}}]*?({CPP_STRING})", usb_definitions):
        if match[1] in usb_ids and not match[1].startswith("generic_"):
            yield usb, usb_source.count("\n", 0, match.start(2)) + 1, json.loads(match[2]), "HID"

    smbus_source = remove_comments(archive.read(smbus).decode("utf-8"))
    match = re.search(r"json rgb_fusion_2_smbus_motherboards\[\]\s*=\s*\{([^}]+)\}", smbus_source)
    if match is None:
        raise ValueError("Gigabyte Fusion 2 SMBus model table was not found")
    for model in re.finditer(CPP_STRING, match[1]):
        line = smbus_source.count("\n", 0, match.start(1) + model.start()) + 1
        yield smbus, line, json.loads(model[0]), "SMBus"


def vendor_for(name):
    folded = name.casefold()
    return next((brand for brand in BRAND_PREFIXES if folded.startswith(brand.casefold() + " ")), None)


def model_row(name, path, line, url, connection, device_type, alias=None):
    row = {"name": " ".join(name.split())}
    for key, value in (("vendor", vendor_for(name)), ("type", device_type), ("connection", connection)):
        if value:
            row[key] = value
    if alias:
        row["aliases"] = [alias]
    row["source"] = f"{url}/{path}#L{line}"
    return row


def unique_models(rows):
    grouped = defaultdict(list)
    for row in rows:
        grouped[row["name"].casefold()].append(row)
    for _, matches in sorted(grouped.items()):
        best = dict(matches[0])
        for key in ("connection", "type"):
            if len({match[key] for match in matches if key in match}) > 1:
                best.pop(key, None)
        aliases = sorted({alias for match in matches for alias in match.get("aliases", ())})
        if aliases:
            best["aliases"] = aliases
        yield best


def build_catalog(source_zip):
    lock, exclusions = patch_exclusions()
    revision = lock["revision"]
    asset = f"source/openrgb-headless-{revision}-source.zip"
    assets = json_object((COMPONENT / "distribution-assets.json").read_bytes())
    pin = next((entry["sha256"] for entry in assets["files"] if entry["path"] == asset), None)
    file_pin(source_zip, pin)

    blob = f"{lock['repository']}/blob/{revision}"
    rows = []
    with ZipFile(source_zip) as archive:
        header = archive.read("DetectionManager.h").decode("utf-8")
        macros = {
            macro
            for macro in re.findall(r"^#define (REGISTER_\w+)\(name,", header, re.M)
            if not macro.endswith("_DISABLED") and not macro.startswith("REGISTER_DYNAMIC_HID_")
        }
        macros.add("REGISTER_WOOTING_DETECTOR")
        for path in sorted(archive.namelist()):
            if windows_controller(path, exclusions):
                source = archive.read(path).decode("utf-8-sig")
                for line, name, connection, device_type, alias in detector_names(source, macros):
                    rows.append(model_row(name, path, line, blob, connection, device_type, alias))
        for path, line, name, connection in gigabyte_board_names(archive):
            rows.append(model_row(f"Gigabyte {name}", path, line, blob, connection, "Motherboard"))
    return {
        "revision": revision,
        "source": f"{lock['repository']}/tree/{revision}",
        "devices": list(unique_models(rows)),
    }


def render(catalog):
    # Keep one model per line so adding or changing a model is one diff line.
    models = ",\n".join(f"    {json.dumps(model, ensure_ascii=False)}" for model in catalog["devices"])
    return (
        f'{{\n  "revision": {json.dumps(catalog["revision"])},\n'
        f'  "source": {json.dumps(catalog["source"])},\n  "devices": [\n{models}\n  ]\n}}\n'
    ).encode("utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source_zip", type=Path, help="hash-pinned OpenRGB component source archive")
    parser.add_argument("--check", action="store_true", help="compare with the checked-in catalog")
    args = parser.parse_args()
    catalog = build_catalog(args.source_zip)
    data = render(catalog)
    if args.check:
        if OUTPUT.read_bytes() != data:
            parser.exit(1, "OpenRGB catalog does not match the pinned source\n")
    else:
        OUTPUT.parent.mkdir(parents=True, exist_ok=True)
        OUTPUT.write_bytes(data)
    print(f"{len(catalog['devices'])} OpenRGB names from {catalog['revision']}")


if __name__ == "__main__":
    main()
