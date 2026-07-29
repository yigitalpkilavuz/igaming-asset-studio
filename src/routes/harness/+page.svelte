<script lang="ts">
  /**
   * Component render harness — mounts one app component with a mocked Tauri backend so it can be
   * driven headlessly (see tests/visual/*.spec.ts, Playwright WebKit). Never shipped to users;
   * client-only (+page.ts sets ssr=false). URL: /harness?c=GamePreview&fixture=lightning
   */
  import { installMock, audioAsset, audioConfig } from "$lib/harness/fixtures";
  import type { AssetDescriptor, GameConfig } from "$lib/ipc";
  import GamePreview from "$lib/components/preview/GamePreview.svelte";
  import AudioBench from "$lib/components/producer/AudioBench.svelte";
  import SoundStudio from "$lib/components/sound/SoundStudio.svelte";
  import FontStudio from "$lib/components/fonts/FontStudio.svelte";
  import SettingsModal from "$lib/components/SettingsModal.svelte";

  const params = new URLSearchParams(window.location.search);
  const component = params.get("c") ?? "GamePreview";
  const fixture = params.get("fixture") ?? "lightning";

  // Install the mock IPC synchronously, before the target component mounts (its onMount calls
  // commands.getProject/… which now hit the mock).
  installMock(fixture);
</script>

<div class="harness-root" data-component={component} data-fixture={fixture}>
  {#if component === "GamePreview"}
    <GamePreview gameId="mock" />
  {:else if component === "AudioBench"}
    <AudioBench
      gameId="mock"
      asset={audioAsset as unknown as AssetDescriptor}
      config={audioConfig as unknown as GameConfig}
    />
  {:else if component === "SoundStudio"}
    <SoundStudio gameId="mock" />
  {:else if component === "FontStudio"}
    <FontStudio gameId="mock" />
  {:else if component === "SettingsModal"}
    <SettingsModal />
  {:else}
    <p class="unknown">Unknown component: <code>{component}</code></p>
  {/if}
</div>

<style>
  :global(html, body) {
    margin: 0;
    height: 100%;
  }
  .harness-root {
    position: fixed;
    inset: 0;
    z-index: 9999; /* cover the app masthead the root +layout.svelte still renders around us */
    background: var(--void, #0a0d12);
  }
  .unknown {
    padding: 2rem;
    font-family: sans-serif;
  }
</style>
