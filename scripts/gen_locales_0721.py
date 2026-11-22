#!/usr/bin/env python3
"""One-shot generator for the seed locale catalogs (W3 migration).

Migrates the legacy ``res/languages/*.toml`` strings into the family's
JSON layout ``res/i18n/locales/<lang>/common.json`` for the 12 locale
dirs (11 family languages + the retained Italian). Run from the repo
root:  python3 scripts/gen_locales_0721.py

The generated files are committed; this script exists so the migration
mapping stays reviewable. It OVERWRITES the common.json files.
"""
from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "res" / "i18n" / "locales"

LANGS = [
    "ar", "de", "en", "es", "fr", "it", "ja", "ko", "pt", "ru",
    "zh-Hans", "zh-Hant",
]

T: dict[str, dict] = {}

T["en"] = {
    "app": {"name": "0721 Resource"},
    "nav": {"portal": "Portal", "users": "Users", "settings": "Settings"},
    "actions": {
        "upload": "Upload", "refresh": "Refresh", "delete": "Delete",
        "copyLink": "Copy link", "copied": "Link copied", "cancel": "Cancel",
        "save": "Save", "saved": "Saved", "create": "Create",
        "confirm": "Confirm", "retry": "Retry", "close": "Close",
        "loadMore": "Load more", "logout": "Sign out", "add": "Add",
        "remove": "Remove",
    },
    "status": {
        "loading": "Loading", "error": "Something went wrong",
        "empty": "Nothing here yet", "admin": "Admin", "member": "Member",
        "total": "{count} in total", "version": "Version {version}",
    },
    "auth": {
        "title": "Sign in", "subtitle": "Continue to 0721",
        "failed": "Invalid username or password",
    },
    "portal": {
        "title": "Portal",
        "empty": "No files yet — add the first one with the + button",
        "uploading": "Uploading {name}",
        "uploadFailed": "Upload failed",
        "deleteConfirm": "Delete this file? Its public link stops working.",
        "openOriginal": "Open original",
    },
    "users": {
        "title": "Users", "empty": "No users",
        "createTitle": "Create user", "username": "Username",
        "password": "Password", "displayName": "Display name", "role": "Role",
        "created": "Created", "deleteConfirm": "Delete user {name}?",
        "conflict": "That username is already taken.",
    },
    "settings": {
        "title": "Site settings", "saved": "Settings saved",
        "saveFailed": "Saving failed",
        "strictHint": "Saving replaces the whole configuration document.",
        "sectionPortal": "Portal", "sectionRouter": "Router",
        "sectionUpload": "Upload",
        "titleSuffix": "Site title suffix", "footerBanner": "Footer banner",
        "language": "Preferred language", "timezone": "Timezone offset (hours)",
        "mediaEntryPath": "Media entry path",
        "limitReferrerHost": "Referrer host allow-list",
        "imageSizeLimit": "Upload size limit",
        "webpAutoConvert": "Auto-convert images to WebP",
        "useSourceFileName": "Keep the source file name",
        "bannerText": "Text", "bannerUrl": "Link (optional)",
        "addBanner": "Add banner entry",
    },
    "chrome": {
        "settings": "Settings", "theme": "Theme", "language": "Language",
        "about": "About", "signOut": "Sign out",
    },
}

# zh-Hans — baseline #2, seeded from res/languages/zh_hans.toml.
T["zh-Hans"] = {
    "app": {"name": "0721 资源站"},
    "nav": {"portal": "门户", "users": "用户", "settings": "配置"},
    "actions": {
        "upload": "上传", "refresh": "刷新", "delete": "删除",
        "copyLink": "复制链接", "copied": "链接已复制", "cancel": "取消",
        "save": "保存", "saved": "已保存", "create": "创建",
        "confirm": "确认", "retry": "重试", "close": "关闭",
        "loadMore": "加载更多", "logout": "退出登录", "add": "添加",
        "remove": "移除",
    },
    "status": {
        "loading": "加载中", "error": "出了点问题",
        "empty": "这里还什么都没有", "admin": "管理员", "member": "用户",
        "total": "共 {count} 项", "version": "版本 {version}",
    },
    "auth": {
        "title": "登录", "subtitle": "继续前往 0721",
        "failed": "用户名或密码错误",
    },
    "portal": {
        "title": "图片",
        "empty": "还没有文件——点右上角加号上传第一个",
        "uploading": "正在上传 {name}",
        "uploadFailed": "上传失败",
        "deleteConfirm": "删除这个文件？公开链接将随之失效。",
        "openOriginal": "查看原图",
    },
    "users": {
        "title": "用户", "empty": "暂无用户",
        "createTitle": "新建用户", "username": "用户名",
        "password": "密码", "displayName": "显示名称", "role": "角色",
        "created": "创建时间", "deleteConfirm": "删除用户 {name}？",
        "conflict": "该用户名已被占用。",
    },
    "settings": {
        "title": "站点配置", "saved": "配置已保存",
        "saveFailed": "保存失败",
        "strictHint": "保存会整体替换配置文档。",
        "sectionPortal": "门户", "sectionRouter": "路由",
        "sectionUpload": "上传",
        "titleSuffix": "站点标题", "footerBanner": "页脚文本",
        "language": "首选语言", "timezone": "时区偏移（小时）",
        "mediaEntryPath": "媒体路由入口",
        "limitReferrerHost": "来源域名白名单",
        "imageSizeLimit": "上传大小限制",
        "webpAutoConvert": "自动转换为 WebP 格式",
        "useSourceFileName": "使用源文件名",
        "bannerText": "文本", "bannerUrl": "链接（可选）",
        "addBanner": "添加页脚条目",
    },
    "chrome": {
        "settings": "设置", "theme": "主题", "language": "语言",
        "about": "关于", "signOut": "退出登录",
    },
}

T["zh-Hant"] = {
    "app": {"name": "0721 資源站"},
    "nav": {"portal": "門戶", "users": "用戶", "settings": "配置"},
    "actions": {
        "upload": "上傳", "refresh": "重新整理", "delete": "刪除",
        "copyLink": "複製連結", "copied": "連結已複製", "cancel": "取消",
        "save": "儲存", "saved": "已儲存", "create": "建立",
        "confirm": "確認", "retry": "重試", "close": "關閉",
        "loadMore": "載入更多", "logout": "登出", "add": "新增",
        "remove": "移除",
    },
    "status": {
        "loading": "載入中", "error": "發生錯誤",
        "empty": "這裡還沒有內容", "admin": "管理員", "member": "用戶",
        "total": "共 {count} 項", "version": "版本 {version}",
    },
    "auth": {
        "title": "登入", "subtitle": "前往 0721",
        "failed": "用戶名或密碼錯誤",
    },
    "portal": {
        "title": "圖片",
        "empty": "還沒有檔案——點右上角加號上傳第一個",
        "uploading": "正在上傳 {name}",
        "uploadFailed": "上傳失敗",
        "deleteConfirm": "刪除這個檔案？公開連結將失效。",
        "openOriginal": "檢視原圖",
    },
    "users": {
        "title": "用戶", "empty": "暫無用戶",
        "createTitle": "新增用戶", "username": "用戶名",
        "password": "密碼", "displayName": "顯示名稱", "role": "角色",
        "created": "建立時間", "deleteConfirm": "刪除用戶 {name}？",
        "conflict": "該用戶名已被使用。",
    },
    "settings": {
        "title": "站點配置", "saved": "配置已儲存",
        "saveFailed": "儲存失敗",
        "strictHint": "儲存會整體替換配置文件。",
        "sectionPortal": "門戶", "sectionRouter": "路由",
        "sectionUpload": "上傳",
        "titleSuffix": "網站標題", "footerBanner": "頁腳文本",
        "language": "首選語言", "timezone": "時區偏移（小時）",
        "mediaEntryPath": "媒體路由入口",
        "limitReferrerHost": "來源網域白名單",
        "imageSizeLimit": "上傳大小限制",
        "webpAutoConvert": "自動轉換為 WebP 格式",
        "useSourceFileName": "使用原始檔名",
        "bannerText": "文字", "bannerUrl": "連結（可選）",
        "addBanner": "新增頁腳條目",
    },
    "chrome": {
        "settings": "設定", "theme": "主題", "language": "語言",
        "about": "關於", "signOut": "登出",
    },
}

# ja — seeded from res/languages/ja.toml.
T["ja"] = {
    "app": {"name": "0721 リソース"},
    "nav": {"portal": "ポータル", "users": "ユーザー", "settings": "設定"},
    "actions": {
        "upload": "アップロード", "refresh": "更新", "delete": "削除",
        "copyLink": "リンクをコピー", "copied": "リンクをコピーしました",
        "cancel": "キャンセル", "save": "保存", "saved": "保存しました",
        "create": "作成", "confirm": "確認", "retry": "再試行",
        "close": "閉じる", "loadMore": "さらに読み込む", "logout": "ログアウト",
        "add": "追加", "remove": "削除",
    },
    "status": {
        "loading": "読み込み中", "error": "問題が発生しました",
        "empty": "まだ何もありません", "admin": "管理者", "member": "ユーザー",
        "total": "全 {count} 件", "version": "バージョン {version}",
    },
    "auth": {
        "title": "ログイン", "subtitle": "0721 へ続行",
        "failed": "ユーザー名またはパスワードが違います",
    },
    "portal": {
        "title": "画像",
        "empty": "ファイルがありません。右上の + から最初の 1 件をアップロード",
        "uploading": "{name} をアップロード中",
        "uploadFailed": "アップロードに失敗しました",
        "deleteConfirm": "このファイルを削除しますか？公開リンクは失効します。",
        "openOriginal": "オリジナルを開く",
    },
    "users": {
        "title": "ユーザー", "empty": "ユーザーがいません",
        "createTitle": "ユーザー作成", "username": "ユーザー名",
        "password": "パスワード", "displayName": "表示名", "role": "役割",
        "created": "作成日時", "deleteConfirm": "ユーザー {name} を削除しますか？",
        "conflict": "そのユーザー名は既に使われています。",
    },
    "settings": {
        "title": "サイト設定", "saved": "設定を保存しました",
        "saveFailed": "保存に失敗しました",
        "strictHint": "保存すると設定ドキュメント全体を置き換えます。",
        "sectionPortal": "ポータル", "sectionRouter": "ルーター",
        "sectionUpload": "アップロード",
        "titleSuffix": "サイトタイトル", "footerBanner": "フッターバナー",
        "language": "既定の言語", "timezone": "タイムゾーンオフセット（時間）",
        "mediaEntryPath": "メディアルーターエントリ",
        "limitReferrerHost": "リファラーホストのホワイトリスト",
        "imageSizeLimit": "アップロードサイズ制限",
        "webpAutoConvert": "WebP 形式へ自動変換する",
        "useSourceFileName": "元のファイル名を使用する",
        "bannerText": "テキスト", "bannerUrl": "リンク（任意）",
        "addBanner": "バナー項目を追加",
    },
    "chrome": {
        "settings": "設定", "theme": "テーマ", "language": "言語",
        "about": "このアプリについて", "signOut": "ログアウト",
    },
}

# ko — seeded from res/languages/ko.toml.
T["ko"] = {
    "app": {"name": "0721 리소스"},
    "nav": {"portal": "포털", "users": "사용자", "settings": "설정"},
    "actions": {
        "upload": "업로드", "refresh": "새로 고침", "delete": "삭제",
        "copyLink": "링크 복사", "copied": "링크를 복사했습니다",
        "cancel": "취소", "save": "저장", "saved": "저장되었습니다",
        "create": "만들기", "confirm": "확인", "retry": "다시 시도",
        "close": "닫기", "loadMore": "더 보기", "logout": "로그아웃",
        "add": "추가", "remove": "제거",
    },
    "status": {
        "loading": "로딩 중", "error": "문제가 발생했습니다",
        "empty": "아직 아무것도 없습니다", "admin": "관리자", "member": "사용자",
        "total": "총 {count}개", "version": "버전 {version}",
    },
    "auth": {
        "title": "로그인", "subtitle": "0721으로 이동",
        "failed": "사용자 이름 또는 비밀번호가 틀렸습니다",
    },
    "portal": {
        "title": "이미지",
        "empty": "파일이 없습니다. 오른쪽 위의 + 로 첫 파일을 업로드하세요",
        "uploading": "{name} 업로드 중",
        "uploadFailed": "업로드 실패",
        "deleteConfirm": "이 파일을 삭제할까요? 공개 링크도 함께失效합니다.".replace("失效", "무효화"),
        "openOriginal": "원본 열기",
    },
    "users": {
        "title": "사용자", "empty": "사용자가 없습니다",
        "createTitle": "사용자 만들기", "username": "사용자 이름",
        "password": "비밀번호", "displayName": "표시 이름", "role": "역할",
        "created": "생성일", "deleteConfirm": "사용자 {name}을(를) 삭제할까요?",
        "conflict": "이미 사용 중인 사용자 이름입니다.",
    },
    "settings": {
        "title": "사이트 설정", "saved": "설정이 저장되었습니다",
        "saveFailed": "저장 실패",
        "strictHint": "저장하면 설정 문서 전체가 교체됩니다.",
        "sectionPortal": "포털", "sectionRouter": "라우터",
        "sectionUpload": "업로드",
        "titleSuffix": "사이트 제목", "footerBanner": "푸터 배너",
        "language": "기본 언어", "timezone": "시간대 오프셋(시간)",
        "mediaEntryPath": "미디어 라우터 경로",
        "limitReferrerHost": "리퍼러 호스트 제한",
        "imageSizeLimit": "업로드 크기 제한",
        "webpAutoConvert": "WebP 자동 변환",
        "useSourceFileName": "원본 파일 이름 사용",
        "bannerText": "텍스트", "bannerUrl": "링크(선택)",
        "addBanner": "배너 항목 추가",
    },
    "chrome": {
        "settings": "설정", "theme": "테마", "language": "언어",
        "about": "정보", "signOut": "로그아웃",
    },
}

# de — seeded from res/languages/de.toml.
T["de"] = {
    "app": {"name": "0721 Ressourcen"},
    "nav": {"portal": "Portal", "users": "Benutzer", "settings": "Konfiguration"},
    "actions": {
        "upload": "Hochladen", "refresh": "Aktualisieren", "delete": "Löschen",
        "copyLink": "Link kopieren", "copied": "Link kopiert",
        "cancel": "Abbrechen", "save": "Speichern", "saved": "Gespeichert",
        "create": "Erstellen", "confirm": "Bestätigen", "retry": "Erneut versuchen",
        "close": "Schließen", "loadMore": "Mehr laden", "logout": "Abmelden",
        "add": "Hinzufügen", "remove": "Entfernen",
    },
    "status": {
        "loading": "Laden", "error": "Etwas ist schiefgelaufen",
        "empty": "Noch nichts hier", "admin": "Admin", "member": "Benutzer",
        "total": "{count} insgesamt", "version": "Version {version}",
    },
    "auth": {
        "title": "Anmelden", "subtitle": "Weiter zu 0721",
        "failed": "Ungültiger Benutzername oder falsches Passwort",
    },
    "portal": {
        "title": "Bilder",
        "empty": "Noch keine Dateien — die erste über + hochladen",
        "uploading": "{name} wird hochgeladen",
        "uploadFailed": "Hochladen fehlgeschlagen",
        "deleteConfirm": "Diese Datei löschen? Der öffentliche Link stoppt dann.",
        "openOriginal": "Original öffnen",
    },
    "users": {
        "title": "Benutzer", "empty": "Keine Benutzer",
        "createTitle": "Benutzer erstellen", "username": "Benutzername",
        "password": "Passwort", "displayName": "Anzeigename", "role": "Rolle",
        "created": "Erstellt", "deleteConfirm": "Benutzer {name} löschen?",
        "conflict": "Dieser Benutzername ist bereits vergeben.",
    },
    "settings": {
        "title": "Site-Konfiguration", "saved": "Einstellungen gespeichert",
        "saveFailed": "Speichern fehlgeschlagen",
        "strictHint": "Speichern ersetzt das gesamte Konfigurationsdokument.",
        "sectionPortal": "Portal", "sectionRouter": "Router",
        "sectionUpload": "Upload",
        "titleSuffix": "Seitentitel", "footerBanner": "Fußzeilentext",
        "language": "Bevorzugte Sprache", "timezone": "Zeitzonen-Offset (Stunden)",
        "mediaEntryPath": "Medien-Routeneinstieg",
        "limitReferrerHost": "Referrer-Host-Whitelist",
        "imageSizeLimit": "Upload-Größenbeschränkung",
        "webpAutoConvert": "Bilder automatisch in WebP konvertieren",
        "useSourceFileName": "Quelldateinamen beibehalten",
        "bannerText": "Text", "bannerUrl": "Link (optional)",
        "addBanner": "Banner-Eintrag hinzufügen",
    },
    "chrome": {
        "settings": "Einstellungen", "theme": "Design", "language": "Sprache",
        "about": "Über", "signOut": "Abmelden",
    },
}

# fr — seeded from res/languages/fr.toml.
T["fr"] = {
    "app": {"name": "0721 Ressources"},
    "nav": {"portal": "Portail", "users": "Utilisateurs", "settings": "Configuration"},
    "actions": {
        "upload": "Téléverser", "refresh": "Rafraîchir", "delete": "Supprimer",
        "copyLink": "Copier le lien", "copied": "Lien copié",
        "cancel": "Annuler", "save": "Enregistrer", "saved": "Enregistré",
        "create": "Créer", "confirm": "Confirmer", "retry": "Réessayer",
        "close": "Fermer", "loadMore": "Charger plus", "logout": "Se déconnecter",
        "add": "Ajouter", "remove": "Retirer",
    },
    "status": {
        "loading": "Chargement", "error": "Une erreur est survenue",
        "empty": "Rien ici pour l'instant", "admin": "Admin", "member": "Utilisateur",
        "total": "{count} au total", "version": "Version {version}",
    },
    "auth": {
        "title": "Connexion", "subtitle": "Continuer vers 0721",
        "failed": "Nom d'utilisateur ou mot de passe incorrect",
    },
    "portal": {
        "title": "Images",
        "empty": "Aucun fichier — téléversez le premier avec le bouton +",
        "uploading": "Téléversement de {name}",
        "uploadFailed": "Échec du téléversement",
        "deleteConfirm": "Supprimer ce fichier ? Son lien public cessera de fonctionner.",
        "openOriginal": "Ouvrir l'original",
    },
    "users": {
        "title": "Utilisateurs", "empty": "Aucun utilisateur",
        "createTitle": "Créer un utilisateur", "username": "Nom d'utilisateur",
        "password": "Mot de passe", "displayName": "Nom affiché", "role": "Rôle",
        "created": "Créé le", "deleteConfirm": "Supprimer l'utilisateur {name} ?",
        "conflict": "Ce nom d'utilisateur est déjà pris.",
    },
    "settings": {
        "title": "Configuration du site", "saved": "Paramètres enregistrés",
        "saveFailed": "Échec de l'enregistrement",
        "strictHint": "L'enregistrement remplace tout le document de configuration.",
        "sectionPortal": "Portail", "sectionRouter": "Routeur",
        "sectionUpload": "Téléversement",
        "titleSuffix": "Titre du site", "footerBanner": "Texte du pied de page",
        "language": "Langue préférée", "timezone": "Décalage horaire (heures)",
        "mediaEntryPath": "Entrée du routeur multimédia",
        "limitReferrerHost": "Liste blanche des domaines référents",
        "imageSizeLimit": "Limite de taille de téléversement",
        "webpAutoConvert": "Convertir automatiquement en WebP",
        "useSourceFileName": "Conserver le nom du fichier source",
        "bannerText": "Texte", "bannerUrl": "Lien (facultatif)",
        "addBanner": "Ajouter une entrée de bannière",
    },
    "chrome": {
        "settings": "Paramètres", "theme": "Thème", "language": "Langue",
        "about": "À propos", "signOut": "Se déconnecter",
    },
}

# es — new locale for the family baseline (no legacy TOML).
T["es"] = {
    "app": {"name": "0721 Recursos"},
    "nav": {"portal": "Portal", "users": "Usuarios", "settings": "Configuración"},
    "actions": {
        "upload": "Subir", "refresh": "Actualizar", "delete": "Eliminar",
        "copyLink": "Copiar enlace", "copied": "Enlace copiado",
        "cancel": "Cancelar", "save": "Guardar", "saved": "Guardado",
        "create": "Crear", "confirm": "Confirmar", "retry": "Reintentar",
        "close": "Cerrar", "loadMore": "Cargar más", "logout": "Cerrar sesión",
        "add": "Añadir", "remove": "Quitar",
    },
    "status": {
        "loading": "Cargando", "error": "Algo salió mal",
        "empty": "Aún no hay nada", "admin": "Administrador", "member": "Usuario",
        "total": "{count} en total", "version": "Versión {version}",
    },
    "auth": {
        "title": "Iniciar sesión", "subtitle": "Continuar a 0721",
        "failed": "Nombre de usuario o contraseña incorrectos",
    },
    "portal": {
        "title": "Imágenes",
        "empty": "Aún no hay archivos: sube el primero con el botón +",
        "uploading": "Subiendo {name}",
        "uploadFailed": "Error al subir",
        "deleteConfirm": "¿Eliminar este archivo? Su enlace público dejará de funcionar.",
        "openOriginal": "Abrir original",
    },
    "users": {
        "title": "Usuarios", "empty": "Sin usuarios",
        "createTitle": "Crear usuario", "username": "Nombre de usuario",
        "password": "Contraseña", "displayName": "Nombre visible", "role": "Rol",
        "created": "Creado", "deleteConfirm": "¿Eliminar al usuario {name}?",
        "conflict": "Ese nombre de usuario ya está en uso.",
    },
    "settings": {
        "title": "Configuración del sitio", "saved": "Configuración guardada",
        "saveFailed": "Error al guardar",
        "strictHint": "Guardar reemplaza todo el documento de configuración.",
        "sectionPortal": "Portal", "sectionRouter": "Enrutador",
        "sectionUpload": "Subida",
        "titleSuffix": "Título del sitio", "footerBanner": "Texto del pie de página",
        "language": "Idioma preferido", "timezone": "Desfase horario (horas)",
        "mediaEntryPath": "Ruta de entrada de medios",
        "limitReferrerHost": "Lista de dominios de referencia permitidos",
        "imageSizeLimit": "Límite de tamaño de subida",
        "webpAutoConvert": "Convertir imágenes a WebP automáticamente",
        "useSourceFileName": "Usar el nombre del archivo original",
        "bannerText": "Texto", "bannerUrl": "Enlace (opcional)",
        "addBanner": "Añadir entrada de banner",
    },
    "chrome": {
        "settings": "Ajustes", "theme": "Tema", "language": "Idioma",
        "about": "Acerca de", "signOut": "Cerrar sesión",
    },
}

# it — legacy locale retained as the 12th dir (not in the family 11).
T["it"] = {
    "app": {"name": "0721 Risorse"},
    "nav": {"portal": "Portale", "users": "Utenti", "settings": "Configurazione"},
    "actions": {
        "upload": "Carica", "refresh": "Aggiorna", "delete": "Elimina",
        "copyLink": "Copia link", "copied": "Link copiato",
        "cancel": "Annulla", "save": "Salva", "saved": "Salvato",
        "create": "Crea", "confirm": "Conferma", "retry": "Riprova",
        "close": "Chiudi", "loadMore": "Carica altro", "logout": "Esci",
        "add": "Aggiungi", "remove": "Rimuovi",
    },
    "status": {
        "loading": "Caricamento", "error": "Qualcosa è andato storto",
        "empty": "Ancora niente qui", "admin": "Amministratore", "member": "Utente",
        "total": "{count} in totale", "version": "Versione {version}",
    },
    "auth": {
        "title": "Accedi", "subtitle": "Continua su 0721",
        "failed": "Nome utente o password non validi",
    },
    "portal": {
        "title": "Immagini",
        "empty": "Nessun file — carica il primo con il pulsante +",
        "uploading": "Caricamento di {name}",
        "uploadFailed": "Caricamento non riuscito",
        "deleteConfirm": "Eliminare questo file? Il link pubblico smetterà di funzionare.",
        "openOriginal": "Apri originale",
    },
    "users": {
        "title": "Utenti", "empty": "Nessun utente",
        "createTitle": "Crea utente", "username": "Nome utente",
        "password": "Password", "displayName": "Nome visualizzato", "role": "Ruolo",
        "created": "Creato", "deleteConfirm": "Eliminare l'utente {name}?",
        "conflict": "Questo nome utente è già in uso.",
    },
    "settings": {
        "title": "Configurazione del sito", "saved": "Impostazioni salvate",
        "saveFailed": "Salvataggio non riuscito",
        "strictHint": "Il salvataggio sostituisce l'intero documento di configurazione.",
        "sectionPortal": "Portale", "sectionRouter": "Router",
        "sectionUpload": "Caricamento",
        "titleSuffix": "Titolo del sito", "footerBanner": "Testo del piè di pagina",
        "language": "Lingua preferita", "timezone": "Fuso orario (ore)",
        "mediaEntryPath": "Percorso di ingresso multimediale",
        "limitReferrerHost": "Elenco domini di provenienza consentiti",
        "imageSizeLimit": "Limite di dimensione del caricamento",
        "webpAutoConvert": "Converti automaticamente in formato WebP",
        "useSourceFileName": "Usa il nome del file sorgente",
        "bannerText": "Testo", "bannerUrl": "Link (opzionale)",
        "addBanner": "Aggiungi voce banner",
    },
    "chrome": {
        "settings": "Impostazioni", "theme": "Tema", "language": "Lingua",
        "about": "Informazioni", "signOut": "Esci",
    },
}

# pt — new locale for the family baseline.
T["pt"] = {
    "app": {"name": "0721 Recursos"},
    "nav": {"portal": "Portal", "users": "Usuários", "settings": "Configurações"},
    "actions": {
        "upload": "Enviar", "refresh": "Atualizar", "delete": "Excluir",
        "copyLink": "Copiar link", "copied": "Link copiado",
        "cancel": "Cancelar", "save": "Salvar", "saved": "Salvo",
        "create": "Criar", "confirm": "Confirmar", "retry": "Tentar novamente",
        "close": "Fechar", "loadMore": "Carregar mais", "logout": "Sair",
        "add": "Adicionar", "remove": "Remover",
    },
    "status": {
        "loading": "Carregando", "error": "Algo deu errado",
        "empty": "Nada por aqui ainda", "admin": "Administrador", "member": "Usuário",
        "total": "{count} no total", "version": "Versão {version}",
    },
    "auth": {
        "title": "Entrar", "subtitle": "Continuar para o 0721",
        "failed": "Nome de usuário ou senha inválidos",
    },
    "portal": {
        "title": "Imagens",
        "empty": "Nenhum arquivo ainda — envie o primeiro com o botão +",
        "uploading": "Enviando {name}",
        "uploadFailed": "Falha no envio",
        "deleteConfirm": "Excluir este arquivo? O link público deixará de funcionar.",
        "openOriginal": "Abrir original",
    },
    "users": {
        "title": "Usuários", "empty": "Nenhum usuário",
        "createTitle": "Criar usuário", "username": "Nome de usuário",
        "password": "Senha", "displayName": "Nome de exibição", "role": "Função",
        "created": "Criado em", "deleteConfirm": "Excluir o usuário {name}?",
        "conflict": "Esse nome de usuário já está em uso.",
    },
    "settings": {
        "title": "Configurações do site", "saved": "Configurações salvas",
        "saveFailed": "Falha ao salvar",
        "strictHint": "Salvar substitui todo o documento de configuração.",
        "sectionPortal": "Portal", "sectionRouter": "Roteador",
        "sectionUpload": "Envio",
        "titleSuffix": "Título do site", "footerBanner": "Texto do rodapé",
        "language": "Idioma preferido", "timezone": "Fuso horário (horas)",
        "mediaEntryPath": "Caminho de entrada de mídia",
        "limitReferrerHost": "Lista de hosts de referência permitidos",
        "imageSizeLimit": "Limite de tamanho de envio",
        "webpAutoConvert": "Converter imagens para WebP automaticamente",
        "useSourceFileName": "Manter o nome do arquivo original",
        "bannerText": "Texto", "bannerUrl": "Link (opcional)",
        "addBanner": "Adicionar entrada de banner",
    },
    "chrome": {
        "settings": "Ajustes", "theme": "Tema", "language": "Idioma",
        "about": "Sobre", "signOut": "Sair",
    },
}

# ru — seeded from res/languages/ru.toml.
T["ru"] = {
    "app": {"name": "0721 Ресурсы"},
    "nav": {"portal": "Портал", "users": "Пользователи", "settings": "Конфигурация"},
    "actions": {
        "upload": "Загрузить", "refresh": "Обновить", "delete": "Удалить",
        "copyLink": "Копировать ссылку", "copied": "Ссылка скопирована",
        "cancel": "Отмена", "save": "Сохранить", "saved": "Сохранено",
        "create": "Создать", "confirm": "Подтвердить", "retry": "Повторить",
        "close": "Закрыть", "loadMore": "Загрузить ещё", "logout": "Выйти",
        "add": "Добавить", "remove": "Убрать",
    },
    "status": {
        "loading": "Загрузка", "error": "Что-то пошло не так",
        "empty": "Здесь пока ничего нет", "admin": "Админ", "member": "Пользователь",
        "total": "Всего: {count}", "version": "Версия {version}",
    },
    "auth": {
        "title": "Вход", "subtitle": "Продолжить в 0721",
        "failed": "Неверное имя пользователя или пароль",
    },
    "portal": {
        "title": "Изображения",
        "empty": "Файлов пока нет — загрузите первый кнопкой +",
        "uploading": "Загрузка {name}",
        "uploadFailed": "Не удалось загрузить",
        "deleteConfirm": "Удалить этот файл? Публичная ссылка перестанет работать.",
        "openOriginal": "Открыть оригинал",
    },
    "users": {
        "title": "Пользователи", "empty": "Нет пользователей",
        "createTitle": "Создать пользователя", "username": "Имя пользователя",
        "password": "Пароль", "displayName": "Отображаемое имя", "role": "Роль",
        "created": "Создан", "deleteConfirm": "Удалить пользователя {name}?",
        "conflict": "Это имя пользователя уже занято.",
    },
    "settings": {
        "title": "Конфигурация сайта", "saved": "Настройки сохранены",
        "saveFailed": "Не удалось сохранить",
        "strictHint": "Сохранение заменяет весь документ конфигурации.",
        "sectionPortal": "Портал", "sectionRouter": "Маршрутизатор",
        "sectionUpload": "Загрузка",
        "titleSuffix": "Заголовок сайта", "footerBanner": "Текст подвала",
        "language": "Предпочитаемый язык", "timezone": "Смещение часового пояса (часы)",
        "mediaEntryPath": "Медиа-путь маршрутизатора",
        "limitReferrerHost": "Белый список источников",
        "imageSizeLimit": "Лимит размера загрузки",
        "webpAutoConvert": "Автоматически конвертировать в WebP",
        "useSourceFileName": "Использовать исходное имя файла",
        "bannerText": "Текст", "bannerUrl": "Ссылка (необязательно)",
        "addBanner": "Добавить запись баннера",
    },
    "chrome": {
        "settings": "Настройки", "theme": "Тема", "language": "Язык",
        "about": "О программе", "signOut": "Выйти",
    },
}

# ar — new locale for the family baseline (RTL).
T["ar"] = {
    "app": {"name": "0721 موارد"},
    "nav": {"portal": "البوابة", "users": "المستخدمون", "settings": "الإعدادات"},
    "actions": {
        "upload": "رفع", "refresh": "تحديث", "delete": "حذف",
        "copyLink": "نسخ الرابط", "copied": "تم نسخ الرابط",
        "cancel": "إلغاء", "save": "حفظ", "saved": "تم الحفظ",
        "create": "إنشاء", "confirm": "تأكيد", "retry": "إعادة المحاولة",
        "close": "إغلاق", "loadMore": "تحميل المزيد", "logout": "تسجيل الخروج",
        "add": "إضافة", "remove": "إزالة",
    },
    "status": {
        "loading": "جارٍ التحميل", "error": "حدث خطأ ما",
        "empty": "لا يوجد شيء هنا بعد", "admin": "مدير", "member": "مستخدم",
        "total": "{count} في المجموع", "version": "الإصدار {version}",
    },
    "auth": {
        "title": "تسجيل الدخول", "subtitle": "المتابعة إلى 0721",
        "failed": "اسم المستخدم أو كلمة المرور غير صحيحة",
    },
    "portal": {
        "title": "الصور",
        "empty": "لا توجد ملفات بعد — ارفع أول ملف بزر +",
        "uploading": "جارٍ رفع {name}",
        "uploadFailed": "فشل الرفع",
        "deleteConfirm": "حذف هذا الملف؟ سيتوقف الرابط العام عن العمل.",
        "openOriginal": "فتح الأصل",
    },
    "users": {
        "title": "المستخدمون", "empty": "لا يوجد مستخدمون",
        "createTitle": "إنشاء مستخدم", "username": "اسم المستخدم",
        "password": "كلمة المرور", "displayName": "الاسم المعروض", "role": "الدور",
        "created": "تاريخ الإنشاء", "deleteConfirm": "حذف المستخدم {name}؟",
        "conflict": "اسم المستخدم هذا محجوز بالفعل.",
    },
    "settings": {
        "title": "إعدادات الموقع", "saved": "تم حفظ الإعدادات",
        "saveFailed": "فشل الحفظ",
        "strictHint": "الحفظ يستبدل مستند الإعدادات بالكامل.",
        "sectionPortal": "البوابة", "sectionRouter": "الموجّه",
        "sectionUpload": "الرفع",
        "titleSuffix": "لاحقة عنوان الموقع", "footerBanner": "نص التذييل",
        "language": "اللغة المفضلة", "timezone": "إزاحة المنطقة الزمنية (ساعات)",
        "mediaEntryPath": "مسار دخول الوسائط",
        "limitReferrerHost": "قائمة المضيفين المسموح لهم بالإحالة",
        "imageSizeLimit": "حد حجم الرفع",
        "webpAutoConvert": "تحويل الصور إلى WebP تلقائيًا",
        "useSourceFileName": "الاحتفاظ باسم الملف الأصلي",
        "bannerText": "النص", "bannerUrl": "الرابط (اختياري)",
        "addBanner": "إضافة إدخال بانر",
    },
    "chrome": {
        "settings": "الإعدادات", "theme": "المظهر", "language": "اللغة",
        "about": "حول", "signOut": "تسجيل الخروج",
    },
}


def main() -> None:
    for lang in LANGS:
        data = T[lang]
        out_dir = OUT / lang
        out_dir.mkdir(parents=True, exist_ok=True)
        (out_dir / "common.json").write_text(
            json.dumps(data, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )
        print(f"wrote {out_dir / 'common.json'}")


if __name__ == "__main__":
    main()
