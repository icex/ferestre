# Pinned source forks

Ferestre builds the components it modifies directly from forks. Fixes live as commits in those repositories; there is no patch-application step.

| Component | Fork | Purpose |
|---|---|---|
| Xodus | [icex/xodus](https://github.com/icex/xodus/tree/ferestre) | Microsoft sign-in, licences, package downloads and the local service |
| Proton | [icex/Proton-UWP](https://github.com/icex/Proton-UWP/tree/ferestre) | Runtime wrapper and pinned runtime dependencies |
| Wine | [icex/wine](https://github.com/icex/wine/tree/ferestre) | GDK title loading, Windows APIs and game input |
| xgameruntime | [icex/xgameruntime](https://github.com/icex/xgameruntime/tree/ferestre) | Xbox GDK runtime implementation |
| vkd3d-proton | [icex/vkd3d-proton](https://github.com/icex/vkd3d-proton/tree/ferestre) | Direct3D 12 fixes |

[`sources.lock.json`](../sources.lock.json) records full commit hashes, upstream bases and the source files changed relative to those bases. The launcher submodules pin Xodus and Proton. Proton in turn pins Wine and vkd3d-proton; Wine pins xgameruntime. Other, unmodified runtime dependencies retain their existing upstream submodule pins.

The existing `Proton-UWP` default branch is preserved. Ferestre uses its separate `ferestre` branch. Builds use the locked commit, even after that branch advances.

## Build from a checkout

```sh
git submodule sync -- third_party/xodus-cli
git submodule update --init third_party/xodus-cli
scripts/check-sources.sh --remote
cargo build --locked --release --manifest-path third_party/xodus-cli/Cargo.toml -p xodus-cli -p xodus-service
cargo build --locked --release --workspace
```

For a runtime build, initialise the Proton source graph:

```sh
git submodule sync -- third_party/xodus-proton
git submodule update --init third_party/xodus-proton
git -C third_party/xodus-proton submodule sync --recursive
git -C third_party/xodus-proton submodule update --init --recursive
```

The runtime build is large and separate from the launcher. [RECIPES.md](RECIPES.md) covers configuring it. Existing dirty source checkouts should be kept separately; these commands do not authorise resetting a developer's changes.

## Verification and updates

`scripts/check-sources.sh` checks launcher gitlinks against the lock. `--remote` additionally fetches the exact public fork commits without checking out the complete Wine tree and verifies the nested gitlinks and repository URLs. CI runs this check and the pinned client's offline tests. AUR, AppImage and portable builds use the same Xodus commit.

To update a component, commit its changes in its fork. Update its parent's gitlink when applicable, then update the launcher gitlinks, `sources.lock.json` and the AUR client pin together. Regenerate `.SRCINFO` with `makepkg --printsrcinfo`, run the source checks and regression tests, and review the resulting runtime build before publishing a runtime release.

The initial migration reconstructs the former build inputs and verifies their source trees against the imported commits. Changes to `.gitmodules` and nested gitlinks select those forks; they do not add new Wine/runtime behaviour. The previous runtime binaries remain usable.

Component licences remain with their source repositories. The source lock travels with installed launcher resources, so binary downloads identify the exact corresponding source. See [NOTICE](../NOTICE).
