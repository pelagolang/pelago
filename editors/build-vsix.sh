#!/bin/sh
# Packs editors/vscode into a .vsix (a zip) with no tools beyond `zip`.
# Usage: editors/build-vsix.sh   ->   editors/dist/pelago-<version>.vsix
set -e
here=$(cd "$(dirname "$0")" && pwd)
src="$here/vscode"
ver=$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' "$src/package.json" | head -1)
tmp=$(mktemp -d)
mkdir -p "$tmp/extension" "$here/dist"
cp -r "$src"/. "$tmp/extension/"
cat > "$tmp/[Content_Types].xml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension=".json" ContentType="application/json"/><Default Extension=".md" ContentType="text/markdown"/><Default Extension=".vsixmanifest" ContentType="text/xml"/></Types>
XML
cat > "$tmp/extension.vsixmanifest" <<XML
<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011">
  <Metadata>
    <Identity Language="en-US" Id="pelago" Version="$ver" Publisher="pelagolang"/>
    <DisplayName>Pelago</DisplayName>
    <Description xml:space="preserve">Syntax highlighting and snippets for the Pelago programming language (.pel)</Description>
    <Categories>Programming Languages,Snippets,Themes</Categories>
  </Metadata>
  <Installation><InstallationTarget Id="Microsoft.VisualStudio.Code"/></Installation>
  <Dependencies/>
  <Assets>
    <Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true"/>
  </Assets>
</PackageManifest>
XML
out="$here/dist/pelago-$ver.vsix"
rm -f "$out"
(cd "$tmp" && zip -qr "$out" '[Content_Types].xml' extension.vsixmanifest extension)
rm -rf "$tmp"
echo "$out"
