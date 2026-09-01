import {
  call,
  type PluginInfo,
  type PluginMutationResult,
  type PluginSnapshot,
} from "../api";

class PluginsStore {
  snapshot = $state<PluginSnapshot | null>(null);

  async refresh(): Promise<PluginSnapshot> {
    const snapshot = await call<PluginSnapshot>("plugin_list");
    this.snapshot = snapshot;
    return snapshot;
  }

  apply(result: PluginMutationResult): PluginMutationResult {
    this.snapshot = result.snapshot;
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
}

export const plugins = new PluginsStore();
