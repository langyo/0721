import { HkBadge } from "@celestia-island/hikari";
import { FileAudio, FileText, FileVideo, Folder, ImageIcon, LogIn, ShieldOff, Unplug, FileQuestion } from "lucide-vue-next";
import { computed, defineComponent, h, ref, type PropType } from "vue";
import { useI18n } from "vue-i18n";

import type { FileDto } from "@/types";
import { formatSize } from "@/composables/formatSize";
import { useMediaPlaceholderLadder } from "@/composables/mediaPlaceholderLadder";

import "./FileDetailView.scss";

/**
 * The embedded file detail — the pane every list surface (files, portal,
 * timeline) shows beside its roster when a row is picked. Render-only:
 * no selection state of its own (the pane's open/close is the host's),
 * no actions of its own (hosts stamp theirs through the actions slot).
 *
 * The image preview runs the R-M② placeholder ladder: a failed `/media`
 * load tries ONE session refresh (P2a's merged ladder); refresh refused
 * → 「请重新登录」 (the same copy for never-signed-in and expired);
 * refresh OK → the retry's status decides: 403 「无读取权限」,
 * 503 「存储不可用」, 404 「不存在」.
 */
export default defineComponent({
  name: "FileDetailView",
  props: {
    file: { type: Object as PropType<FileDto>, required: true },
  },
  setup(props) {
    const { t } = useI18n();
    const ladder = useMediaPlaceholderLadder();

    const kindIcon = computed(() => {
      if (props.file.kind === "folder") return Folder;
      if (props.file.mime.startsWith("audio/")) return FileAudio;
      if (props.file.mime.startsWith("video/")) return FileVideo;
      if (props.file.mime.startsWith("text/")) return FileText;
      return ImageIcon;
    });

    const face = computed(() => ladder.faceOf(props.file.url));

    const faceIcon = computed(() => {
      switch (face.value?.kind) {
        case "sign-in": return LogIn;
        case "forbidden": return ShieldOff;
        case "unavailable": return Unplug;
        case "missing": return FileQuestion;
        default: return null;
      }
    });

    const faceKey = computed(() => {
      switch (face.value?.kind) {
        case "sign-in": return "common.media.signin";
        case "forbidden": return "common.media.forbidden";
        case "unavailable": return "common.media.unavailable";
        case "missing": return "common.media.missing";
        default: return null;
      }
    });

    /** The R-M② ladder on a failed load; a later successful load
     *  clears the face through `onImageLoad`. */
    async function onImageError(): Promise<void> {
      await ladder.resolve(
        props.file.url,
        async () => {
          try {
            const response = await fetch("/api/v1/auth/refresh", {
              method: "POST",
              credentials: "same-origin",
            });
            return response.ok;
          } catch {
            return false;
          }
        },
        async (url) => {
          try {
            const retry = await fetch(url, { credentials: "same-origin" });
            return retry.status;
          } catch {
            return 0;
          }
        },
      );
    }

    function onImageLoad(): void {
      ladder.clear(props.file.url);
    }

    function formatWhen(iso: string): string {
      const date = new Date(iso);
      return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
    }

    return () => (
      <div class="file-detail">
        <div class="preview">
          {props.file.kind === "image" && !face.value ? (
            <img
              src={props.file.url}
              alt={props.file.original_name || props.file.name}
              loading="lazy"
              onError={onImageError}
              onLoad={onImageLoad}
            />
          ) : props.file.kind === "image" && face.value ? (
            <div class="media-placeholder" data-testid={`media-placeholder-${face.value.kind}`}>
              {h(faceIcon.value!, { size: 28, class: "placeholder-icon" })}
              <span class="placeholder-text">{t(faceKey.value!)}</span>
            </div>
          ) : (
            h(kindIcon.value, { size: 64, class: "kind-icon" })
          )}
        </div>

        <div class="meta">
          <p class="name" title={props.file.original_name || props.file.name}>{props.file.original_name || props.file.name}</p>
          <div class="rows">
            <span class="row">
              <span class="label">{t("common.files.size")}</span>
              {props.file.kind !== "folder"
                ? <span class="mono">{formatSize(props.file.size)}</span>
                : <span>—</span>}
            </span>
            <span class="row">
              <span class="label">{t("common.files.uploaded")}</span>
              <span>{formatWhen(props.file.created_at)}</span>
            </span>
            <span class="row">
              <span class="label">{t("common.files.kind")}</span>
              {props.file.kind === "folder"
                ? <HkBadge variant="info" pill>{t("common.files.kindFolder")}</HkBadge>
                : <HkBadge variant="muted" pill mono>{props.file.mime}</HkBadge>}
            </span>
          </div>
        </div>
      </div>
    );
  },
});
