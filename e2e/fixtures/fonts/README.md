# Visual test fonts

These files are used only by Playwright screenshots and are not shipped in the app.

- `visual-regular.woff2` and `visual-bold.woff2`: subsets of Noto Sans CJK SC, renamed to NodeFlare Visual Regular/Bold. Input: Debian fonts-noto-cjk, Simplified Chinese face (index 2) of the Regular/Bold TTCs.
- `visual-flags.woff2`: regional-indicator/flag subset of Noto Color Emoji, renamed NodeFlare Visual Flags.
- Licenses and upstream copyright notices are retained in `LICENSE-cjk.txt` and `LICENSE-emoji.txt` (SIL Open Font License 1.1).

The CJK subsets include ASCII and characters in frontend TS/TSX and the bootstrap fixture at the time of generation. If a screenshot adds characters outside the subset, regenerate fonts with fontTools and review the updated baselines; do not rely on system fallback fonts. The font files themselves pin the rendering input across local and CI runs.
