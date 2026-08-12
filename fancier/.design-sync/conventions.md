# PidgeIoT design conventions

This design system ships **styles and tokens only** — no importable JS components (the product's own components are Rust/Dioxus). Build every screen from your own JSX styled with the class vocabulary below. All styling comes from `styles.css` → `_ds_bundle.css` (full DaisyUI 5.7 component classes + PidgeIoT's two custom themes + chart tokens). Read that file when unsure whether a class exists.

## Theme setup (required)

Wrap each design in a themed root; without it you get the system-preference default:

```jsx
<div data-theme="light" className="min-h-screen bg-base-100 text-base-content">…</div>
```

`data-theme="light"` and `data-theme="dark"` are the only two themes. Always give the root `bg-base-100 text-base-content`. Both themes are hand-tuned oklch — never hardcode hex/gray-* colors; use semantic classes so designs work in both themes.

## Styling idiom: DaisyUI classes + Tailwind utilities

Semantic color families (each `X` has `bg-X`, `text-X`, `border-X`, and a paired `X-content` for text on that background): `primary`, `secondary`, `accent`, `info`, `success`, `warning`, `error`, plus surfaces `base-100` (page), `base-200` (raised), `base-300` (borders/wells) and `base-content` (ink).

**Never use `neutral` (`bg-neutral`, `btn-neutral`, `badge-neutral`, `status-neutral`)** — in these themes neutral is white-on-white (light) / black-on-black (dark) and renders invisible. Use `badge-ghost` / `btn-ghost` or `base-*` instead.

Component classes (verified present; modify with `btn-primary`-style color suffixes and `btn-xs|sm|md|lg` sizes):

| Family | Classes |
|---|---|
| Actions | `btn` (+`btn-primary/secondary/accent/ghost/outline/error`), `dropdown`, `swap`, `modal` + `modal-box` |
| Data display | `card` + `card-body` + `card-title`, `badge`, `stat` + `stat-title` + `stat-value`, `table` + `table-zebra`, `kbd`, `avatar`, `status`, `skeleton`, `loading`, `progress`, `radial-progress`, `mockup-code`, `collapse` |
| Feedback | `alert` (+`alert-error` etc.), `toast`, `tooltip` |
| Forms | `fieldset`, `label`, `input`, `select`, `textarea`, `checkbox`, `radio`, `toggle`, `range`, `file-input` |
| Navigation | `navbar`, `menu`, `tabs` + `tab`, `breadcrumbs`, `steps`, `link`, `join`, `drawer`, `footer`, `hero`, `indicator`, `divider` |

Layout glue (spacing, flex/grid, typography sizes) is plain Tailwind utilities. Corner radii come from theme tokens automatically (`--radius-box` boxes ≈ cards/modals, `--radius-field` inputs/buttons, `--radius-selector` small controls) — don't override with `rounded-*`.

## Charts

For any data visualization use the chart tokens (theme-aware): surface `var(--chart-surface)`, gridlines `var(--chart-grid)`, axes `var(--chart-axis)`, labels `var(--chart-ink-primary)` / `var(--chart-ink-secondary)`, and series colors `var(--chart-series-1)` … `var(--chart-series-8)` **in that exact order** — the ordering is a validated color-vision-deficiency-safe palette, so assign series 1→8 in sequence, never cherry-picked.

## Idiomatic example

```jsx
<div data-theme="dark" className="min-h-screen bg-base-100 text-base-content p-6">
  <div className="card bg-base-200">
    <div className="card-body gap-4">
      <h2 className="card-title">Sensor fleet</h2>
      <div className="stat p-0">
        <div className="stat-title">Online devices</div>
        <div className="stat-value text-primary">128</div>
      </div>
      <div className="flex items-center gap-2">
        <span className="badge badge-ghost">unknown</span>
        <span className="badge badge-success">reporting</span>
        <button className="btn btn-primary btn-sm">Provision device</button>
      </div>
    </div>
  </div>
</div>
```
