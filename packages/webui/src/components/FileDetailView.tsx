import { HkBadge } from "@celestia-island/hikari";
import { FileAudio, FileText, FileVideo, Folder, ImageIcon } from "lucide-vue-next";
import { computed, defineComponent, h, type PropType } from "vue";
import { useI18n } from "vue-i18n";

import type { FileDto } from "@/types";
import { formatSize } from "@/composables/formatSize";

import "./FileDetailView.scss";

/**
 * The embedded file detail — the pane every list surface (files, portal,
 * timeline) shows beside its roster when a row is picked. Render-only:
 * no selection state of its own (the pane's open/close is the host's),
 * no actions of its own (hosts stamp theirs through the actions slot).
 */
export default defineComponent({
  name: "FileDetailView",
  props: {
    file: { type: Object as PropType<FileDto>, required: true },
  },
  setup(props) {
    const { t } = useI18n();

    const kindIcon = computed(() => {
      if (props.file.kind === "folder") return Folder;
      if (props.file.mime.startsWith("audio/")) return FileAudio;
      if (props.file.mime.startsWith("video/")) return FileVideo;
      if (props.file.mime.startsWith("text/")) return FileText;
      return ImageIcon;
    });

    function formatWhen(iso: string): string {
      const date = new Date(iso);
      return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
    }

    return () => (
      <div class="file-detail">
        <div class="preview">
          {props.file.kind === "image" ? (
            <img
              src={props.file.url}
              alt={props.file.original_name || props.file.name}
              loading="lazy"
            />
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
