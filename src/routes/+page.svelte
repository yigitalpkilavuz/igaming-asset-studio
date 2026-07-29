<script lang="ts">
  import { appState } from "$lib/stores/app.svelte";
  import "$lib/stores/theme.svelte"; // applies the persisted theme on startup
  import ProjectsList from "$lib/components/ProjectsList.svelte";
  import Producer from "$lib/components/producer/Producer.svelte";
  import AnimateSection from "$lib/components/animate/AnimateSection.svelte";
  import GamePreview from "$lib/components/preview/GamePreview.svelte";
  import SoundStudio from "$lib/components/sound/SoundStudio.svelte";
  import JobToasts from "$lib/components/JobToasts.svelte";
</script>

{#if appState.section === "library"}
  <ProjectsList />
{:else if appState.section === "animate" && appState.gameId}
  {#key appState.gameId}
    <AnimateSection gameId={appState.gameId} />
  {/key}
{:else if appState.section === "sound" && appState.gameId}
  {#key appState.gameId}
    <SoundStudio gameId={appState.gameId} />
  {/key}
{:else if appState.section === "preview" && appState.gameId}
  {#key appState.gameId}
    <GamePreview gameId={appState.gameId} />
  {/key}
{:else}
  {#key appState.gameId ?? "∅new"}
    <Producer gameId={appState.gameId} assetKey={appState.produceAssetKey} />
  {/key}
{/if}

<JobToasts />
