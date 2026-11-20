#!/bin/bash
set -euo pipefail

# Invoked by pkexec. Never change the global AppArmor/user namespace policy.
uid=${1:?Missing user ID}
runtime_root=${2:?Missing umu directory}
[[ $(id -u) == 0 && $uid =~ ^[0-9]+$ && $uid != 0 ]] || exit 1
caller=${PKEXEC_UID:-${SUDO_UID:-}}
[[ $caller == "$uid" ]] || { echo "User ID does not match the authorizing user" >&2; exit 1; }
[[ $runtime_root =~ ^/[[:alnum:]_./:\ +~-]+$ ]] || { echo "Unsupported umu path" >&2; exit 1; }
runtime_root=$(readlink -f -- "$runtime_root")
[[ $runtime_root =~ ^/[[:alnum:]_./:\ +~-]+$ ]] || exit 1
[[ -d $runtime_root && $(stat -c %u -- "$runtime_root") == "$uid" ]] || exit 1

profile_name="hlsr-umu-$uid"
target="/etc/apparmor.d/$profile_name"
if [[ -L $target ]] || { [[ -e $target ]] && ! grep -q '^# Managed by HLSR\.$' "$target"; }; then
  echo "Preserving an AppArmor profile not managed by HLSR: $target" >&2
  exit 1
fi
temporary=$(mktemp "/etc/apparmor.d/.$profile_name.XXXXXX")
trap 'rm -f -- "$temporary"' EXIT
cat > "$temporary" <<PROFILE
# Managed by HLSR.
abi <abi/4.0>,
include <tunables/global>
profile $profile_name "$runtime_root/steamrt*/pressure-vessel/libexec/steam-runtime-tools-0/srt-bwrap" flags=(unconfined) {
  userns,
}
PROFILE
apparmor_parser -Q "$temporary"
install -m 0644 "$temporary" "$target"
apparmor_parser -r "$target"
