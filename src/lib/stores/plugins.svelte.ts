import {
  call,
  type PluginInfo,
  type PluginMutationResult,
  type PluginSnapshot,
} from "../api";

class PluginsStore {
  snapshot = $state<PluginSnapshot | null>(null);

  get fileDragEnabled(): boolean {
    return this.snapshot?.file_drag_provider != null;
  }

  private publish(snapshot: PluginSnapshot): PluginSnapshot {
    this.snapshot = snapshot;
    if (typeof document !== "undefined") {
      document.documentElement.classList.toggle(
        "file-drag-plugin-enabled",
        snapshot.file_drag_provider != null,
      );
    }
    return snapshot;
  }

  async refresh(): Promise<PluginSnapshot> {
    const snapshot = await call<PluginSnapshot>("plugin_list");
    return this.publish(snapshot);
  }

  apply(result: PluginMutationResult): PluginMutationResult {
    this.publish(result.snapshot);
    return result;
  }

  async install(): Promise<PluginMutationResult> {
    return this.apply(await call<PluginMutationResult>("plugin_choose_install"));
  }

  async setEnabled(plugin: PluginInfo, enabled: boolean): Promise<PluginMutationResult> {
    return this.apply(
      await call<PluginMutationResult>("plugin_set_enabled", {
        pluginId: plugin.id,
        enabled,
      }),
    );
  }

  async setSetting(
    pluginId: string,
    key: string,
    value: unknown,
  ): Promise<PluginMutationResult> {
    return this.apply(
      await call<PluginMutationResult>("plugin_set_setting", {
        pluginId,
        key,
        value,
      }),
    );
  }

  async uninstall(plugin: PluginInfo, removeData: boolean): Promise<PluginMutationResult> {
    return this.apply(
      await call<PluginMutationResult>("plugin_uninstall", {
        pluginId: plugin.id,
        removeData,
      }),
    );
  }

  async groupingMigration(action: "install" | "dismiss"): Promise<PluginMutationResult> {
    return this.apply(
      await call<PluginMutationResult>("plugin_grouping_migration", { action }),
    );
  }

  async approveGrants(plugin: PluginInfo): Promise<PluginMutationResult> {
    return this.apply(
      await call<PluginMutationResult>("plugin_approve_grants", { pluginId: plugin.id }),
    );
  }

  async invoke(pluginId: string, commandId: string, input: unknown): Promise<unknown> {
    return call<unknown>("plugin_invoke", { pluginId, commandId, input });
  }
}

export const plugins = new PluginsStore();
