<script setup lang="ts">
import { HkToast } from "@celestia-island/hikari";
</script>

<!--
  The root is one router-view plus the family toast outlet: the login
  route renders bare (its own centered card), every authed route renders
  inside AppShell (the family admin shell). Mounting HkToast here means
  any surface — shell or view — can useToast() without per-page outlets.
-->

<template>
  <RouterView />
  <HkToast />
</template>
