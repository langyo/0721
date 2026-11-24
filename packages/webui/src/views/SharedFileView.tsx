import { HkButton, HkEmptyState } from "@celestia-island/hikari";
import { Download } from "lucide-vue-next";
import { defineComponent, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute } from "vue-router";
import { RouterLink } from "vue-router";

import * as files from "@/api/files";

import "./SharedFileView.scss";

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

    return () => (
      <div class="shared-page">
        <nav class="shared-nav"><RouterLink to="/login">0721</RouterLink></nav>
        {error.value ? (
          <HkEmptyState boxed title={t("common.files.fileNotAvailable")} />
        ) : meta.value ? (
          <div class="shared-card">
            <h2>{meta.value?.file_name}</h2>
            {meta.value?.allow_preview ? <p class="preview-label">{t("common.files.preview")}</p> : null}
            <p class="meta">{meta.value?.file_size} B · {meta.value?.kind}</p>
            {/* Inline preview: image / text / pdf / media */}
            {meta.value?.allow_preview && meta.value?.mime.startsWith("image/") ? (
              <img src={downloadUrl} alt={meta.value?.file_name} class="preview-img" />
            ) : meta.value?.allow_preview && meta.value?.mime.startsWith("video/") ? (
              <video src={downloadUrl} controls class="preview-media" />
            ) : meta.value?.allow_preview && meta.value?.mime.startsWith("audio/") ? (
              <audio src={downloadUrl} controls class="preview-audio" />
            ) : meta.value?.allow_preview && meta.value?.mime === "application/pdf" ? (
              <iframe src={downloadUrl} class="preview-pdf" />
            ) : meta.value?.allow_preview && (meta.value?.mime.startsWith("text/") || meta.value?.mime === "application/json") ? (
              <pre class="preview-text">{textContent.value ?? "Loading…"}</pre>
            ) : null}
            <HkButton variant="primary" aria-label={t("common.files.download")} onClick={() => download()}>
              {{
                icon: () => <Download size={16} />,
                default: () =>                     t("common.files.download"),
              }}
            </HkButton>
          </div>
        ) : null}
      </div>
    );
  },
});
