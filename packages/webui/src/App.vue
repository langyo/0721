<script setup lang="ts">
// The root shell is one router-view: the login route renders bare (its
// own centered card), every authed route renders inside AppShell, which
// owns the HkAdminHeader action row and the settings dialog.
</script>

<template>
  <RouterView />
</template>
