# Themes & Layouts Guide

DockPanel ships with 6 built-in themes and multiple layout options. Every theme works with every layout.

## Available Themes

| Theme | Description |
|-------|-------------|
| **Nexus Dark** | Deep dark background with vibrant accent colors |
| **Nexus Light** | Clean light background with sharp contrast |
| **Ocean** | Blue-tinted dark theme inspired by deep sea |
| **Forest** | Green-tinted dark theme with natural tones |
| **Sunset** | Warm orange/red dark theme |
| **Midnight** | Pure black AMOLED-friendly theme |

## Changing Themes

### From the Panel

1. Click the theme icon in the top navigation bar
2. Select a theme from the dropdown
3. The theme applies immediately -- no page reload needed

Your theme preference is saved per user and persists across sessions.

### Keyboard Shortcut

Press `Ctrl+Shift+T` to cycle through themes.

## Layout Options

| Layout | Description |
|--------|-------------|
| **Sidebar** | Traditional sidebar navigation on the left |
| **Compact** | Narrow icon-only sidebar that expands on hover |
| **Top** | Horizontal navigation bar at the top |

### Changing Layout

1. Go to **Settings** > **Appearance**
2. Select your preferred layout
3. The layout applies immediately

## Customization

### Card Depth

Cards in the dashboard support adjustable depth (shadow intensity):

- **Flat**: No shadow
- **Subtle**: Light shadow for minimal depth
- **Raised**: Standard shadow
- **Elevated**: Strong shadow for prominent cards

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
