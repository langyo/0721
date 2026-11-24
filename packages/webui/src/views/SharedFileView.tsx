import { defineComponent, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, RouterLink } from "vue-router";
import { HkButton, HkEmptyState } from "@celestia-island/hikari";
import { Download } from "lucide-vue-next";

import * as files from "@/api/files";

import "./SharedFileView.css";

/** The public share landing: no auth, one token, one file. */
export default defineComponent({
  name: "SharedFileView",
  setup() {
    const { t } = useI18n();
    const route = useRoute();
    const token = route.params.token as string;
    const meta = ref<files.ShareMeta | null>(null);
    const error = ref(false);
    const downloadUrl = files.shareDownloadUrl(token);
    const textContent = ref<string | null>(null);

    onMounted(() => {
      files.shareMeta(token)
        .then((m) => {
          meta.value = m;
          if (
            m.allow_preview &&
            (m.mime.startsWith("text/") || m.mime === "application/json")
          ) {
            void fetch(downloadUrl)
              .then((r) => r.text())
              .then((txt) => { textContent.value = txt; })
              .catch(() => undefined);
          }
        })
        .catch(() => { error.value = true; });
    });

    function download() {
      window.location.href = downloadUrl;
    }

    const preview = () => {
      if (!(meta.value?.allow_preview)) return null;
      const mime = meta.value.mime;
      if (mime.startsWith("image/")) {
        return <img src={downloadUrl} alt={meta.value.file_name} class="preview-img" />;
      }
      if (mime.startsWith("video/")) {
        return <video src={downloadUrl} controls class="preview-media" />;
      }
      if (mime.startsWith("audio/")) {
        return <audio src={downloadUrl} controls class="preview-audio" />;
      }
      if (mime === "application/pdf") {
        return <iframe src={downloadUrl} class="preview-pdf" />;
      }
      if (mime.startsWith("text/") || mime === "application/json") {
        return <pre class="preview-text">{textContent.value ?? "Loading…"}</pre>;
      }
      return null;
    };

    return () => (
      <div class="shared-page">
        <nav class="shared-nav"><RouterLink to="/login">0721</RouterLink></nav>
        {error.value
          ? <HkEmptyState boxed title={t("common.files.fileNotAvailable")} />
          : (meta.value && (
            <div class="shared-card">
              <h2>{meta.value.file_name}</h2>
              {meta.value.allow_preview && <p class="preview-label">{t("common.files.preview")}</p>}
              <p class="meta">{meta.value.file_size} B · {meta.value.kind}</p>
              {preview()}
              <HkButton variant="primary" aria-label={t("common.files.download")} onClick={download}>
                {{ icon: () => <Download size={16} /> }}
                {t("common.files.download")}
              </HkButton>
            </div>
          ))}
      </div>
    );
  },
});
