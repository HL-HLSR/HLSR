"use strict";

const fs = require("fs");
const path = require("path");

// Stores the device session. With a system keyring (DPAPI, Keychain, KWallet,
// GNOME Keyring) the payload is encrypted through safeStorage. Without one,
// as on a Steam Deck in Game Mode or a desktop with KWallet turned off, it is
// kept as plain JSON readable only by the owner, like Steam or gh do: the
// token is per-device and can be revoked server-side.
class SecureSession {
  constructor(file, storage) {
    this.file = file;
    this.storage = storage;
  }

  encrypted() {
    try {
      return !!this.storage && this.storage.isEncryptionAvailable() &&
        (!this.storage.getSelectedStorageBackend || this.storage.getSelectedStorageBackend() !== "basic_text");
    } catch (error) {
      return false;
    }
  }

  read() {
    try {
      if (fs.statSync(this.file).size > 256 * 1024) return null;
      const envelope = JSON.parse(fs.readFileSync(this.file, "utf8"));
      if (typeof envelope.cipher === "string") {
        if (!this.encrypted()) return null;
        return JSON.parse(this.storage.decryptString(Buffer.from(envelope.cipher, "base64")));
      }
      return envelope.plain && typeof envelope.plain === "object" ? envelope.plain : null;
    } catch (error) {
      return null;
    }
  }

  write(data) {
    const envelope = this.encrypted()
      ? { version: 1, cipher: this.storage.encryptString(JSON.stringify(data)).toString("base64") }
      : { version: 1, plain: data };
    fs.mkdirSync(path.dirname(this.file), { recursive: true });
    const temp = this.file + ".tmp";
    try { fs.unlinkSync(temp); } catch (error) { /* No leftover from an interrupted write. */ }
    fs.writeFileSync(temp, JSON.stringify(envelope), { mode: 0o600 });
    fs.renameSync(temp, this.file);
  }

  clear() {
    try { fs.unlinkSync(this.file); } catch (error) { /* A missing session is already cleared. */ }
  }
}

module.exports = SecureSession;
