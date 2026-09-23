#!/bin/bash
# Build inputs must already exist; Node and Rust are needed only on the builder.
set -euo pipefail
umask 022

repository_root="$(cd "$(dirname "$0")/.." && pwd -P)"
cd "$repository_root"

if [[ "$(uname -s)" != Darwin || "$(uname -m)" != x86_64 ]]; then
  printf '%s\n' 'This Intel preview must be packaged and tested on an Intel Mac.' >&2
  exit 1
fi

for required_file in extension/dist/manifest.json extension/lib/native-client.mjs companion/target/release/naab-companion scripts/install-macos.command docs/macos-testing.md; do
  if [[ ! -f "$required_file" ]]; then
    printf 'Missing build input: %s\n' "$required_file" >&2
    exit 1
  fi
done

source_commit="$(git rev-parse HEAD)"
if [[ -n "${GITHUB_SHA:-}" && "$GITHUB_SHA" != "$source_commit" ]]; then
  printf '%s\n' 'Checkout commit differs from the workflow source commit.' >&2
  exit 1
fi
git diff --quiet
git diff --cached --quiet

output_directory="${1:-companion/target/macos-preview}"
mkdir -p "$output_directory"
output_directory="$(cd "$output_directory" && pwd -P)"
staging_directory="$(mktemp -d "$output_directory/.naab-package.XXXXXX")"
trap 'rm -rf "$staging_directory"' EXIT
package_directory="$staging_directory/NAAB-Mac-Preview"
mkdir -p "$package_directory/extension" "$package_directory/companion"
cp -R extension/dist/. "$package_directory/extension/"
cp companion/target/release/naab-companion "$package_directory/companion/naab-companion"
cp scripts/install-macos.command "$package_directory/Install.command"
cp docs/macos-testing.md "$package_directory/README.md"
chmod 755 "$package_directory/Install.command" "$package_directory/companion/naab-companion"

# This is a local preview signature, not Developer ID signing or notarization.
/usr/bin/lipo "$package_directory/companion/naab-companion" -verify_arch x86_64
codesign --force --sign - --timestamp=none "$package_directory/companion/naab-companion"
codesign --verify --strict --verbose=2 "$package_directory/companion/naab-companion"

# Exercise the exact signed executable that will be distributed, using the
# built extension's NativeClient and the real framed stdin/stdout transport.
NAAB_BINARY="$package_directory/companion/naab-companion" node --test tests/native-integration.test.mjs

NAAB_PACKAGE_DIRECTORY="$package_directory" node --input-type=module <<'NODE'
import { readFileSync, writeFileSync, readdirSync, lstatSync } from 'node:fs';
import { join, relative } from 'node:path';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { NativeClient } from './extension/lib/native-client.mjs';
import { nativeTransport } from './tests/native-transport.mjs';

const root = process.env.NAAB_PACKAGE_DIRECTORY;
const command = (program, args) => execFileSync(program, args, { encoding: 'utf8' }).trim();
const binary = join(root, 'companion/naab-companion');
const architectures = command('/usr/bin/lipo', [binary, '-archs']);
if (architectures !== 'x86_64') throw new Error(`Expected an Intel-only executable, got: ${architectures}`);
const loadCommands = command('/usr/bin/otool', ['-l', binary]);
const macOSBuildCommands = loadCommands.split(/(?=Load command \d+\s*\n)/).filter(block => /^\s*cmd LC_(?:BUILD_VERSION|VERSION_MIN_MACOSX)\s*$/m.test(block));
if (macOSBuildCommands.length !== 1) throw new Error('Expected exactly one macOS minimum-version load command.');
const buildCommand = macOSBuildCommands[0];
if (/^\s*cmd LC_BUILD_VERSION\s*$/m.test(buildCommand) && !/^\s*platform (?:1|MACOS)\s*$/m.test(buildCommand)) throw new Error('Executable does not target macOS.');
const minimumMacOS = buildCommand.match(/^\s*(?:minos|version) (\d+(?:\.\d+){0,2})\s*$/m)?.[1];
const sdkVersion = buildCommand.match(/^\s*sdk (\d+(?:\.\d+){0,2})\s*$/m)?.[1];
if (!minimumMacOS || !sdkVersion) throw new Error('Could not read the executable minimum macOS and SDK versions.');
const versionParts = value => value.split('.').map(Number).concat([0, 0]).slice(0, 3);
const requiredParts = versionParts(minimumMacOS);
const userParts = versionParts('13.3.1');
const firstDifference = requiredParts.findIndex((part, index) => part !== userParts[index]);
if (firstDifference !== -1 && requiredParts[firstDifference] > userParts[firstDifference]) throw new Error(`Executable needs macOS ${minimumMacOS}; the requested Mac runs 13.3.1.`);
const libraryLines = command('/usr/bin/otool', ['-L', binary]).split('\n').slice(1).filter(line => line.trim());
const linkedLibraries = libraryLines.map(line => {
  const path = line.match(/^\s+(.+?)\s+\(compatibility version [^)]+\)$/)?.[1];
  if (!path || !(path.startsWith('/usr/lib/') || path.startsWith('/System/Library/'))) throw new Error(`Non-system or unrecognized linked library: ${line.trim()}`);
  return path;
});
if (linkedLibraries.length === 0) throw new Error('Executable library inspection returned no dependencies.');
if (/^\s*cmd LC_RPATH\s*$/m.test(loadCommands)) throw new Error('Unexpected runtime search path in standalone companion.');
console.log(`Packaged Mach-O: ${architectures}; minimum macOS ${minimumMacOS}; SDK ${sdkVersion}`);
console.log(`Linked system libraries: ${linkedLibraries.join(', ')}`);
const extension = JSON.parse(readFileSync(join(root, 'extension/manifest.json'), 'utf8'));
const packageVersion = JSON.parse(readFileSync('extension/package.json', 'utf8')).version;
if (extension.version !== packageVersion) throw new Error('Extension bundle version is stale.');
const metadata = JSON.parse(command('cargo', ['metadata', '--locked', '--no-deps', '--format-version', '1', '--manifest-path', 'companion/Cargo.toml']));
const companion = metadata.packages.find(pkg => pkg.name === 'naab-companion');
if (!companion) throw new Error('Companion package metadata is missing.');
const status = await new NativeClient(nativeTransport(binary)).status();
if (status.companionVersion !== companion.version) throw new Error('Companion executable version is stale.');
const files = [];
function walk(directory) {
  for (const name of readdirSync(directory).sort()) {
    const path = join(directory, name);
    const stat = lstatSync(path);
    if (stat.isSymbolicLink()) throw new Error(`Unexpected package symlink: ${path}`);
    if (stat.isDirectory()) walk(path);
    else if (stat.isFile()) files.push({ path: relative(root, path).split('\\').join('/'), bytes: stat.size, sha256: createHash('sha256').update(readFileSync(path)).digest('hex') });
    else throw new Error(`Unexpected package entry: ${path}`);
  }
}
walk(root);
const info = {
  formatVersion: 1,
  product: 'Not Another Ad Blocker Mac preview',
  extensionVersion: extension.version,
  companionVersion: companion.version,
  companionProtocolVersion: status.protocolVersion,
  sourceCommit: command('git', ['rev-parse', 'HEAD']),
  sourceRef: process.env.GITHUB_REF_NAME ?? command('git', ['branch', '--show-current']),
  repository: process.env.GITHUB_REPOSITORY ?? null,
  builtAt: new Date().toISOString(),
  builder: { macOSVersion: command('sw_vers', ['-productVersion']), macOSBuild: command('sw_vers', ['-buildVersion']), architecture: command('uname', ['-m']), runnerImageVersion: process.env.ImageVersion ?? null, rust: command('rustc', ['--version']), node: process.version },
  target: { architecture: architectures, minimumInstallerMacOS: '12', binaryDeploymentTarget: process.env.MACOSX_DEPLOYMENT_TARGET ?? 'compiler default', minimumBinaryMacOS: minimumMacOS, sdkVersion, linkedLibraries, checkedAgainstMacOS: '13.3.1', signing: 'ad-hoc; not notarized' },
  files,
};
writeFileSync(join(root, 'BUILD-INFO.json'), JSON.stringify(info, null, 2) + '\n');
NODE

extension_version="$(node -p "JSON.parse(require('fs').readFileSync('extension/dist/manifest.json','utf8')).version")"
archive_name="NAAB-Mac-Preview-${extension_version}-Intel-${source_commit:0:12}.tar.gz"
COPYFILE_DISABLE=1 tar -czf "$output_directory/$archive_name" -C "$staging_directory" NAAB-Mac-Preview
(cd "$output_directory" && shasum -a 256 "$archive_name" > SHA256SUMS.txt)
printf 'Created %s\n' "$output_directory/$archive_name"
cat "$output_directory/SHA256SUMS.txt"
