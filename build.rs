// SPDX-License-Identifier: GPL-3.0-only
fn main() {
    glib_build_tools::compile_resources(&["data"], "data/resources.gresource.xml", "comfy-gnome.gresource");
}
