#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Recopie la logique partagée du logiciel de bureau dans l'API web
(`../Bellepoule Web/bellepoule-web-api`), qui doit rester autonome :

- `bp-core`            : copie intégrale de `crates/bp-core/src` ;
- `marshaller`         : logique d'organisation (sans l'interface GTK `ui/`
                         ni `main.rs`), compilée sans l'option `gui` ;
- `bp-net`             : seulement le type `Message` (le reste du réseau
                         n'est pas utilisé par le web) ;
- traductions          : `resources/translations` (mêmes textes que le
                         logiciel : noms de phases, de tableaux…).

Usage : python tools/sync_web.py
"""

import os
import re
import shutil

HERE = os.path.dirname(os.path.abspath(__file__))
DESKTOP = os.path.normpath(os.path.join(HERE, ".."))
WEB = os.path.normpath(os.path.join(DESKTOP, "..", "Bellepoule Web", "bellepoule-web-api"))


def copy_tree(src, dst, skip=()):
    if os.path.isdir(dst):
        shutil.rmtree(dst)
    shutil.copytree(src, dst, ignore=lambda d, names: [n for n in names if os.path.join(d, n) in skip])


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


def remove_fn(text, name):
    """Retire `pub fn name(...) { ... }` (avec son commentaire de doc)."""
    m = re.search(r"((?:\n    ///[^\n]*)*)\n    pub fn " + name + r"\b", text)
    if not m:
        return text
    i = text.index("{", m.end())
    depth = 0
    for j in range(i, len(text)):
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
            if depth == 0:
                return text[: m.start()] + text[j + 1 :]
    return text


def main():
    # bp-core
    copy_tree(os.path.join(DESKTOP, "crates", "bp-core", "src"), os.path.join(WEB, "bp-core", "src"))

    # Traductions
    copy_tree(os.path.join(DESKTOP, "resources", "translations"), os.path.join(WEB, "resources", "translations"))

    # marshaller (logique seule)
    src = os.path.join(DESKTOP, "crates", "marshaller", "src")
    dst = os.path.join(WEB, "marshaller", "src")
    copy_tree(src, dst, skip=(os.path.join(src, "ui"), os.path.join(src, "main.rs")))
    lib = open(os.path.join(dst, "lib.rs"), encoding="utf-8").read()
    lib = re.sub(r'#\[cfg\(feature = "gui"\)\]\s*pub mod ui;\s*', "", lib)
    write(os.path.join(dst, "lib.rs"), lib)
    write(
        os.path.join(WEB, "marshaller", "Cargo.toml"),
        """[package]
name = "marshaller"
description = "Organisation (pistes, arbitres, planning) : logique du Marshaller, sans interface"
version.workspace = true
edition.workspace = true
license.workspace = true

[features]
# L'interface GTK n'existe pas dans cette copie.
gui = []

[dependencies]
bp-core = { path = "../bp-core" }
bp-net = { path = "../bp-net" }
chrono = { workspace = true }
roxmltree = { workspace = true }
serde_json = { workspace = true }
sha1 = { workspace = true }
sha2 = "0.10"
aes = "0.8"
cbc = { version = "0.1", features = ["alloc"] }
base64 = "0.22"
rand = { workspace = true }
qrcode = { version = "0.14", default-features = false }
encoding_rs = { workspace = true }
log = { workspace = true }
""",
    )

    # bp-net : type Message seul
    message = open(os.path.join(DESKTOP, "crates", "bp-net", "src", "message.rs"), encoding="utf-8").read()
    for name in ("spread", "recall", "dump"):
        message = remove_fn(message, name)
    write(os.path.join(WEB, "bp-net", "src", "message.rs"), message)
    write(
        os.path.join(WEB, "bp-net", "src", "lib.rs"),
        "//! Messages BellePoule (copie du type `Message` de `bp-net`).\n\npub mod message;\npub use message::Message;\n",
    )
    write(
        os.path.join(WEB, "bp-net", "Cargo.toml"),
        """[package]
name = "bp-net"
description = "Type Message de BellePoule (copie autonome)"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
rand = { workspace = true }
""",
    )
    print("Synchronisé vers", WEB)


if __name__ == "__main__":
    main()
