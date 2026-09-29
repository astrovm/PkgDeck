# Bundled artwork

`logo.svg` is a copy of the repository's application icon
(`assets/io.github.astrovm.PkgDeck.svg`), shown beside the sidebar wordmark.
It carries the project's own artwork, unlike the third-party mark below.
`logo-template.svg` is the same cube in black with shaded faces, for the
macOS menu bar, which tints it to match light and dark menu bars.

# GitHub mark

`github.svg` is the `mark-github-16.svg` icon from GitHub's Octicons. `github.png`
is its bundled raster form for Qt installations without an SVG image decoder:
https://github.com/primer/octicons/blob/main/icons/mark-github-16.svg

`github-dark.svg` changes only the fill color for dark backgrounds.
Both assets are redistributed under the accompanying `OCTICONS-LICENSE`.
The icons and license are embedded through `resources.qrc`; release bundles also
include the license in `usr/share/licenses/pkgdeck`.
