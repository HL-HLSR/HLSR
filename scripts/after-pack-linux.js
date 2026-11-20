"use strict";

const fs = require("fs");
const path = require("path");
const stripLinuxElf = require("./strip-linux-elf");

module.exports = function afterPackLinux(context) {
  stripLinuxElf(context);
  if (context.electronPlatformName !== "linux") return;
  // Match the archive installer's launch mode: extracted files cannot provide
  // a root-owned setuid helper, and Ubuntu 24.04 restricts user namespaces.
  const script = '#!/bin/sh\nROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd) || exit 1\nunset APPIMAGE APPDIR ARGV0 OWD\nexec "$ROOT/hlsr" --no-sandbox "$@"\n';
  fs.writeFileSync(path.join(context.appOutDir, "start"), script, { mode: 0o755 });
};
