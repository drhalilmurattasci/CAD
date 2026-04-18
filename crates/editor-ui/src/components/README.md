# Editor Components

Each menu, toolbar, status bar, workspace region, and panel renderer lives in its own file so the editor shell can grow as a reusable component library instead of one monolithic UI module.

- `menu_bar.rs`: reusable top menu renderer driven by JSON definitions.
- `toolbar.rs`: reusable toolbar renderer driven by action lists.
- `status_bar.rs`: bottom status strip renderer.
- `workspace.rs`: dock-region/tab orchestration.
- `viewport.rs`: scene viewport shell.
- `hierarchy.rs`: entity tree panel.
- `inspector.rs`: selection inspector panel.
- `content_browser.rs`: asset browser panel.
- `console.rs`: logs and commands panel.
- `profiler.rs`: runtime diagnostics panel.
