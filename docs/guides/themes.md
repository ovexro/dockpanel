# Themes & Layouts Guide

DockPanel ships with 6 built-in themes and multiple layout options. Every theme works with every layout.

## Available Themes

| Theme | Description |
|-------|-------------|
| **Terminal** | Hacker aesthetic -- near-black with a green accent |
| **Midnight** | Deep navy, modern (the default) |
| **Ember** | Warm & premium -- dark stone with an orange accent |
| **Clean Dark** | GitHub-dark, rounded |
| **Arctic** | Teal & light |
| **Clean Light** | Modern SaaS, blue |

Four are dark (Terminal, Midnight, Ember, Clean Dark) and two are light (Arctic,
Clean Light).

## Changing Themes

### From the Panel

Either:

- Click the theme button in the top navigation bar to cycle through the six in
  order, or
- Go to **Settings** > **Appearance** and click a theme in the swatch grid.

Either way the theme applies immediately -- no page reload needed.

Your theme choice is stored in the browser (`localStorage`, key `dp-theme`), so
it follows the browser and the device rather than the account. Signing in from a
different browser starts again at the default.

## Layout Options

| Layout | Description |
|--------|-------------|
| **Sidebar** | Traditional sidebar navigation on the left |
| **Compact** | Narrow icon-only sidebar that expands on hover |
| **Topbar** | Horizontal navigation bar at the top |

### Changing Layout

1. Go to **Settings** > **Appearance**
2. Select your preferred layout
3. The layout applies immediately

## Customization

### Progress Bar Glow

Progress bars (backup progress, deploy progress, etc.) feature a subtle glow effect that matches the active theme's accent color.

### Status Indicators

Status indicators (online, offline, degraded) use consistent color coding across all themes:

- **Green**: Operational / healthy
- **Yellow**: Degraded / warning
- **Red**: Down / critical
- **Gray**: Unknown / inactive

## White-Labeling

DockPanel supports custom branding:

1. Go to **Settings** > **Branding**
2. Set:
   - **Panel name**: Custom name shown in the sidebar/header
   - **Logo URL**: Your custom logo
   - **Favicon URL**: Custom browser tab icon
3. Click **Save**

Branding is visible to all users and on the login page.
