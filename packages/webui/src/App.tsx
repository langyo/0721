import { HkToast } from "@celestia-island/hikari";
import { defineComponent } from "vue";
import { RouterView } from "vue-router";

/**
 * The root is one router-view plus the family toast outlet: the login
 * route renders bare (its own centered card), every authed route renders
 * inside AppShell (the family admin shell). Mounting HkToast here means
 * any surface — shell or view — can useToast() without per-page outlets.
 */
export default defineComponent({
  name: "App",
  setup() {
    return () => (
      <>
        <RouterView />
        <HkToast />
      </>
    );
  },
});
