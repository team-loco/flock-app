# Releasing Flock

Flock ships for Apple silicon only: an arm64 `Flock.app` in two variants, default features and
`--features loco`, each zipped and attached to a GitHub Release. Each build updates only from its
own variant (`flock_update::release::Variant`). The app checks the latest release itself and installs it after verifying an ed25519
signature and a SHA-256 checksum.

## Cut a release

From an up-to-date `main` with a clean tree:

```sh
mise run release -- 0.2.0
```

The task bumps `[workspace.package] version` in `Cargo.toml`, refreshes `Cargo.lock`, commits
`Release v0.2.0`, tags `v0.2.0` and pushes the commit and the tag. A version with a pre-release part
(`0.3.0-beta.1`) is published as a GitHub pre-release; Flock offers it only to people who turned
on "Include pre-releases".

Pushing the tag starts `.github/workflows/release.yml` on `macos-latest`:

1. checks that the tag matches the workspace version
2. `mise run package` and `mise run package -- --loco`: each builds and ad-hoc signs
   `dist/Flock.app` with `mise-tasks/bundle`, checks the binary is arm64 and zips it with
   `ditto -c -k --keepParent` as `dist/Flock-<version>-arm64.zip` or
   `dist/Flock-<version>-loco-arm64.zip`
3. `mise run sign -- <zip>` for each zip: writes `<zip>.sha256` (shasum format) and `<zip>.sig`
   (base64 ed25519 signature of the zip), then verifies both with the public key compiled into the
   app. Only this step sees the signing key
4. writes release notes from the commit subjects since the previous `v*` tag
5. `gh release create` with both zips, their checksums and their signatures

## What installed copies do

Flock checks `releases/latest` (or the release list when "Include pre-releases" is on) 20 seconds
after launch and then on the interval from Settings → Updates, and on Flock → Check for Updates…,
⌘⇧P "Flock: Check for updates" or Settings → Check now. A newer release downloads in the background,
is verified (signature and checksum) before anything is written, and is staged beside the running
bundle; the status bar then offers "Restart to update". A copy run with `cargo run` or from a
folder Flock cannot write shows "Download Flock <version>" linking to the release page instead.

## Build and sign locally

```sh
mise run package             # or: mise run package -- --loco
mise run sign -- dist/Flock-0.2.0-arm64.zip
```

`sign` reads the key from `FLOCK_UPDATE_SIGNING_KEY`, or from the login Keychain when the variable is
unset. `target/release/flock-sign verify <zip>` checks a zip against its `.sha256` and `.sig` with
the embedded public key.

## Signing key

- Public key: `flock_update::signature::PUBLIC_KEY` in `crates/flock-update/src/signature.rs`.
- Private key: the repository Actions secret `FLOCK_UPDATE_SIGNING_KEY` (base64 of the 32-byte
  ed25519 seed), with a backup in the maintainer's login Keychain as the generic password
  `dev.flock.app.update-signing`, account `team-loco`:

  ```sh
  security find-generic-password -s dev.flock.app.update-signing -a team-loco -w
  ```

The private key is never committed, printed or written to a file outside a short-lived temp file
that is removed once the secret and the Keychain item are set.

## Rotate the key

Installed copies trust only the key they were built with, so a rotation takes two releases:

1. Generate a new pair: `cargo run -p flock-update --bin flock-sign -- keygen <temp file>` prints
   the new public key and writes the private key to the file (mode 0600, never overwritten).
2. Replace `PUBLIC_KEY` with the new public key and cut a release. This release is still signed
   with the **old** key, so every installed copy accepts it and from then on trusts the new key.
3. Store the new private key: `gh secret set FLOCK_UPDATE_SIGNING_KEY -R team-loco/flock-app <
   <temp file>`, then replace the Keychain item (`security add-generic-password -U -s
   dev.flock.app.update-signing -a team-loco -w "$(cat <temp file>)"`) and delete the file with
   `rm -P`.
4. Releases after that are signed with the new key.

If the old key leaked, skip step 2's grace release: change the key, publish, and tell users to
download the new version by hand, since installed copies will reject it.

## First install and Gatekeeper

The bundle is ad-hoc signed, not notarized. A zip downloaded with a browser carries the quarantine
attribute, so the first launch is blocked.

The no-prompt route is the install script, which downloads with curl (no quarantine), verifies the
`.sha256`, installs `/Applications/Flock.app` and clears the attribute anyway:

```sh
curl -fsSL https://raw.githubusercontent.com/team-loco/flock-app/main/install.sh | sh
```

`FLOCK_LOCO=1` installs the Loco variant, `FLOCK_VERSION` pins a version and `FLOCK_INSTALL_DIR`
changes the target directory.

For a browser download, open Flock from System Settings → Privacy & Security → Open Anyway, or run
`xattr -dr com.apple.quarantine /Applications/Flock.app`. Updates that Flock downloads itself are
not quarantined and launch without a prompt.
