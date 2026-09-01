import type {
  PartialTranslationCatalog,
  TranslationCatalog,
} from "../catalog.types";
import { englishCatalog } from "./en.js";

/**
 * Turkish overrides are key-checked against the English source catalog. The
 * spread is also a deliberate last-resort fallback for future English keys.
 */
const turkishOverrides = {
  "app.name": "Personal Hub",
  "common.all": "Tümü",
  "common.available": "Kullanılabilir",
  "common.cancel": "İptal",
  "common.checking": "Kontrol ediliyor…",
  "common.close": "Kapat",
  "common.delete": "Sil",
  "common.disabled": "Devre dışı",
  "common.enabled": "Etkin",
  "common.loading": "Yükleniyor…",
  "common.none": "Yok",
  "common.off": "Kapalı",
  "common.on": "Açık",
  "common.save": "Kaydet",
  "common.selected": "Seçili",
  "common.settings": "Ayarlar",
  "common.tryAgain": "Yeniden dene",
  "common.unavailable": "Kullanılamıyor",
  "common.working": "İşleniyor…",

  "errorBoundary.eyebrow": "Personal Hub",
  "errorBoundary.title": "Bir şeyler ters gitti.",
  "errorBoundary.description":
    "Pano görüntülenemedi. Yeniden denemek için uygulamayı yeniden başlatın.",
  "errorBoundary.reload": "Panoyu yeniden yükle",

  "header.brandLabel": "Personal Hub",
  "header.brandSubtitle": "Ev sunucusu",
  "header.primaryNavigation": "Ana gezinme",
  "header.dashboard": "Pano",
  "header.searchServices": "Servislerde ara",
  "header.searchShortcut": "Ctrl K",
  "header.openSettings": "Ayarları aç",
  "header.settingsTitle": "Ayarlar",

  "dashboard.panelLabel": "Pano",
  "dashboard.eyebrow": "Pano",
  "dashboard.title": "Evdeki her şey tek bir yerde.",
  "dashboard.description":
    "Ev sunucunuzda çalışan servisler için sakin bir komuta merkezi.",
  "dashboard.currentPhase": "Geçerli uygulama aşaması",
  "dashboard.phaseLabel": "Aşama 7.3",
  "dashboard.phaseDescription": "Görünüm ve diller",
  "dashboard.configurationUnavailableTitle":
    "Servis yapılandırması kullanılamıyor",
  "dashboard.settingsNoticeTitle": "Servis ayarları bildirimi",
  "dashboard.reviewSettings": "Ayarları gözden geçir",
  "dashboard.servicesTitle": "Servisler",
  "dashboard.configurationLoaded": "Kayıtlı servis ayarlarınızdan yüklendi",
  "dashboard.configurationLoading": "Kayıtlı servis ayarları yükleniyor",
  "dashboard.configurationUnavailable": "Yapılandırma kullanılamıyor",
  "dashboard.refreshAutomatically":
    "Durum her 45 saniyede bir otomatik yenilenir.",
  "dashboard.refreshStatus": "Durumu yenile",
  "dashboard.filterByCategory": "Servisleri kategoriye göre filtrele",
  "dashboard.noServicesFound": "Servis bulunamadı",
  "dashboard.noServicesFoundDescription": "Başka bir ad veya kategori deneyin.",
  "dashboard.noEnabledServices": "Etkin servis yok",
  "dashboard.noEnabledServicesDescription":
    "Ayarlar'dan bir servisi etkinleştirin veya ekleyin.",
  "dashboard.footerDescription": "Önce yerel masaüstü kontrol merkezi",
  "dashboard.desktopOnly":
    "Servis görünümleri tarayıcı önizlemesinde değil, Windows masaüstü uygulamasında kullanılabilir.",
  "dashboard.openedInSystemBrowser":
    "{{serviceName}} sistem tarayıcısında açıldı.",
  "dashboard.urlCopied": "{{serviceName}} URL'si kopyalandı.",
  "dashboard.clipboardUnavailable": "Panoya erişilemiyor.",

  "health.noEnabledServices": "Kontrol edilecek etkin servis yok",
  "health.checkingServices": "{{count}} servis kontrol ediliyor…",
  "health.ready": "Sağlık kontrolleri hazır",
  "health.refreshingPrefix": "Yenileniyor",
  "health.onlineCount": "{{count}} çevrimiçi",
  "health.localTlsCount": "{{count}} yerel TLS",
  "health.warningCountOne": "{{count}} uyarı",
  "health.warningCountOther": "{{count}} uyarı",
  "health.offlineCount": "{{count}} çevrimdışı",
  "health.uncheckedCount": "{{count}} kontrol edilmedi",

  "monitoring.connectingEyebrow": "Bağlanıyor",
  "monitoring.connectingTitle": "Glances'a ulaşılıyor",
  "monitoring.connectingDescription": "İlk sunucu örneği bekleniyor.",
  "monitoring.optionalEyebrow": "İsteğe bağlı",
  "monitoring.notConfiguredTitle": "İzleme yapılandırılmamış",
  "monitoring.notConfiguredDescription":
    "CPU, bellek, depolama ve çalışma süresini göstermek için uyumlu bir izleme sağlayıcısı ekleyin.",
  "monitoring.partialEyebrow": "Kısmi",
  "monitoring.partialTitle": "Bazı metrikler kullanılamıyor",
  "monitoring.liveEyebrow": "Canlı",
  "monitoring.onlineTitle": "Sunucu çevrimiçi",
  "monitoring.onlineDescription": "Sistem metrikleri otomatik güncelleniyor.",
  "monitoring.staleEyebrow": "Eski veri",
  "monitoring.interruptedTitle": "Canlı metrikler kesildi",
  "monitoring.interruptedDescription":
    "Son başarılı sunucu örneği gösteriliyor.",
  "monitoring.unavailableEyebrow": "Kullanılamıyor",
  "monitoring.unavailableTitle": "İzleme sağlayıcısı kullanılamıyor",
  "monitoring.unavailableDescription":
    "İzleme örneği yok. Servis erişilebilirliği ayrı olarak kontrol edilir.",
  "monitoring.refresh": "Sunucu metriklerini yenile",
  "monitoring.serverUptime": "Sunucu çalışma süresi",
  "monitoring.lastKnownUptime": "Bilinen son çalışma süresi",
  "monitoring.liveMetrics": "Canlı sunucu metrikleri",
  "monitoring.lastKnownMetrics": "Bilinen son sunucu metrikleri",
  "monitoring.cpu": "CPU",
  "monitoring.memory": "Bellek",
  "monitoring.network": "Ağ",
  "monitoring.storage": "Depolama",
  "monitoring.metricUsage": "{{metric}} kullanımı",
  "monitoring.temperatureUnavailable": "Sıcaklık kullanılamıyor",
  "monitoring.temperature": "{{temperature}} sıcaklık",
  "monitoring.memoryUsage": "{{total}} kapasitenin {{used}} kadarı",
  "monitoring.networkRate": "İndirme · yükleme {{upload}}",
  "monitoring.diskDataUnavailable": "Disk verisi kullanılamıyor",
  "monitoring.diskUsage": "{{name}} · {{total}} kapasitenin {{used}} kadarı",
  "monitoring.volumeCountOne": "{{count}} birim",
  "monitoring.volumeCountOther": "{{count}} birim",
  "monitoring.uptimeUnavailable": "Kullanılamıyor",

  "service.checking": "Kontrol ediliyor",
  "service.notChecked": "Kontrol edilmedi",
  "service.online": "Çevrimiçi · {{latency}}",
  "service.localTls": "Yerel TLS · {{latency}}",
  "service.tlsIssue": "TLS sorunu",
  "service.desktopOnly": "Yalnız masaüstü",
  "service.attention": "Dikkat gerekli",
  "service.timedOut": "Zaman aşımına uğradı",
  "service.httpStatus": "HTTP {{status}}",
  "service.offline": "Çevrimdışı",
  "service.refreshingStatus": "Durum yenileniyor…",
  "service.cardLabel": "{{serviceName}}, {{status}}",
  "service.openInHub": "{{serviceName}} servisini Personal Hub'da aç",
  "service.actions": "{{serviceName}} işlemleri",
  "service.open": "Aç",
  "service.openInNewTab": "Yeni sekmede aç",
  "service.openInSystemBrowser": "Sistem tarayıcısında aç",
  "service.copyUrl": "URL'yi kopyala",
  "service.edit": "Servisi düzenle",
  "service.gridLoading": "Servis yapılandırması yükleniyor",

  "tabs.openViews": "Açık görünümler",
  "tabs.listLabel": "Personal Hub sekmeleri",
  "tabs.dashboard": "Pano",
  "tabs.closeTab": "{{serviceName}} sekmesini kapat",
  "tabs.closeService": "{{serviceName}} servisini kapat",

  "serviceView.panelLabel": "{{serviceName}} servis görünümü",
  "serviceView.unavailable": "Servis görünümü kullanılamıyor",
  "serviceView.ready": "Servis görünümü hazır",
  "serviceView.opening": "Servis açılıyor…",

  "settings.headerEyebrow": "Personal Hub",
  "settings.title": "Ayarlar",
  "settings.description":
    "Görünüm, servisler, arka plan çalışma zamanı ve güvenli entegrasyon kimlik bilgileri.",
  "settings.close": "Ayarları kapat",
  "settings.configurationNotice": "Yapılandırma bildirimi",
  "settings.operationFailed": "İşlem tamamlanamadı",
  "settings.updated": "Ayarlar güncellendi",
  "settings.catalogKicker": "Katalog",
  "settings.servicesTitle": "Servisler",
  "settings.configured": "yapılandırıldı",
  "settings.serviceCount": "{{count}} yapılandırıldı",
  "settings.selectedService": "Seçili servis",
  "settings.addService": "Servis ekle",
  "settings.deleteService": "Sil",
  "settings.unnamedService": "Adsız servis",
  "settings.noCategory": "Kategori yok",
  "settings.serviceId": "Servis kimliği",
  "settings.savedIdHelp":
    "Kimlik bilgileri servis kimliklerine bağlı olduğundan kayıtlı kimlikler değiştirilemez.",
  "settings.newIdHelp":
    "Kararlı, küçük harfli bir kimlik seçin; kimlik bilgileri buna bağlanacağı için kaydettikten sonra değiştirilemez.",
  "settings.name": "Ad",
  "settings.descriptionLabel": "Açıklama",
  "settings.url": "URL",
  "settings.urlPlaceholder": "https://192.168.1.10:8443",
  "settings.urlHelp": "Yalnız HTTP(S). URL'lere kimlik bilgisi eklenemez.",
  "settings.category": "Kategori",
  "settings.icon": "Simge",
  "settings.accent": "Vurgu",
  "settings.accentViolet": "Mor",
  "settings.accentAmber": "Kehribar",
  "settings.accentBlue": "Mavi",
  "settings.accentCyan": "Camgöbeği",
  "settings.accentGreen": "Yeşil",
  "settings.accentOrange": "Turuncu",
  "settings.accentRed": "Kırmızı",
  "settings.accentSlate": "Arduvaz",
  "settings.tlsPolicy": "TLS ilkesi",
  "settings.tlsStrict": "Katı doğrulama",
  "settings.tlsAllowInvalid": "Geçersiz yerel sertifikaya izin ver",
  "settings.tlsHelp": "Gevşetilmiş ilke yalnız özel HTTPS hedefleriyle sınırlıdır.",
  "settings.apiAuthentication": "Otomatik API kimlik doğrulaması",
  "settings.apiAuthenticationHelp":
    "Yalnız eşleşen yerel sağlayıcı bağdaştırıcısı tarafından kullanılır; sırlar kimlik bilgisi kasasında kalır.",
  "settings.browserAuthentication": "Tarayıcı kimlik doğrulaması",
  "settings.browserAuthenticationHelp":
    "Normal oturum açma kalıcı servis sekmesi profilinde kalır. Sayfaya veya giriş formuna parola eklenmez.",
  "settings.allowLocalHttp": "Yerel düz metin HTTP üzerinden kimlik bilgilerine izin ver",
  "settings.allowLocalHttpHelp":
    "Yalnız güvendiğiniz bir geri döngü veya özel ağ servisi için etkinleştirin. HTTPS önerilmeye devam eder.",
  "settings.unsavedChanges": "Kaydedilmemiş katalog değişiklikleri",
  "settings.catalogUpToDate": "Katalog güncel",
  "settings.discardEdits": "Düzenlemeleri at",
  "settings.saving": "Kaydediliyor…",
  "settings.loadingCatalog": "Servis kataloğu yükleniyor",
  "settings.loadingCatalogDescription":
    "Doğrulanmış kullanıcı yapılandırması okunuyor.",
  "settings.noServices": "Yapılandırılmış servis yok",
  "settings.noServicesDescription":
    "Bir servis ekleyin, alanlarını doldurun ve kataloğu kaydedin.",
  "settings.saveEmptyCatalog": "Boş kataloğu kaydet",
  "settings.newServiceName": "Yeni servis",
  "settings.newServiceDescription": "Yerel servis",
  "settings.newServiceCategory": "Diğer",
  "settings.serviceDisabledSuffix": "devre dışı",
  "settings.addedNotice":
    "Taslağa yeni servis eklendi. Kalıcı olması için kaydedin.",
  "settings.removedNotice":
    "{{serviceName}} taslaktan kaldırıldı. Kalıcı olması için kaydedin.",
  "settings.discardedNotice": "Kaydedilmemiş düzenlemeler atıldı.",
  "settings.invalidConfiguration": "Servis yapılandırması geçersiz.",
  "settings.savedNotice": "Servis yapılandırması kaydedildi.",
  "settings.resetConfirmation":
    "Kayıtlı servis kataloğu paketle gelen varsayılanlarla değiştirilsin mi?",
  "settings.restoreConfirmation":
    "Kayıtlı servis kataloğu en son yedeğiyle değiştirilsin mi?",
  "settings.resetNotice": "Paketle gelen varsayılanlar geri yüklendi.",
  "settings.restoreNotice": "Yapılandırma yedeği geri yüklendi.",
  "settings.discardCloseConfirmation":
    "Kaydedilmemiş katalog değişiklikleri atılıp Ayarlar kapatılsın mı?",

  "credentials.kicker": "Windows kimlik bilgisi kasası",
  "credentials.title": "Kimlik bilgileri",
  "credentials.privateBadge": "Sırlar hiçbir zaman arayüze dönmez",
  "credentials.selectService":
    "Kimlik bilgilerini yönetmeden önce bir servis seçin veya ekleyin.",
  "credentials.saveFirst":
    "Kimlik bilgilerini yönetmeden önce katalog düzenlemelerini kaydedin veya atın.",
  "credentials.providerAdapter": "Sağlayıcı bağdaştırıcısı",
  "credentials.automaticAuthentication": "Otomatik kimlik doğrulama",
  "credentials.checkingStored":
    "Kayıtlı kimlik bilgisi servisle kontrol ediliyor.",
  "credentials.readingStatus": "Yerel bağdaştırıcı durumu okunuyor.",
  "credentials.statusUnavailable": "Durum kullanılamıyor: {{error}}",
  "credentials.nativeApi": "Yerel API",
  "credentials.browserPath": "Tarayıcı yolu",
  "credentials.requiredEntries": "Gerekli kasa girdileri",
  "credentials.browserLoginHelp":
    "Tarayıcı oturumu ayrıdır ve kalıcı servis sekmesi profilinde kalır. Otomatik kimlik bilgileri hiçbir zaman web sayfası alanlarına veya giriş formlarına eklenmez.",
  "credentials.validateNow": "Şimdi doğrula",
  "credentials.stored": "Kayıtlı",
  "credentials.notStored": "Kayıtlı değil",
  "credentials.usernamePlaceholder": "Yeni kullanıcı adı girin",
  "credentials.valuePlaceholder": "Yeni değer girin",
  "credentials.deleteStored": "Kayıtlı olanı sil",
  "credentials.storeReplacement": "Yeni değeri kaydet",
  "credentials.requiredField": "{{field}} boş bırakılamaz.",
  "credentials.storedNotice":
    "{{credential}} yerel kimlik bilgisi kasasına kaydedildi.",
  "credentials.deleteConfirmation":
    "{{serviceName}} için kayıtlı {{credential}} silinsin mi? Bu kimlik bilgisi kurtarılamaz.",
  "credentials.removedNotice":
    "{{credential}} yerel kimlik bilgisi kasasından kaldırıldı.",
  "credentials.apiKeyTitle": "API anahtarı",
  "credentials.apiKeyDescription":
    "Servise özgü API üst bilgileri ve entegrasyonları için.",
  "credentials.apiKeyLabel": "API anahtarı",
  "credentials.bearerTokenTitle": "Bearer belirteci",
  "credentials.bearerTokenDescription":
    "Belgelenmiş belirteç tabanlı sağlayıcı API'leri için.",
  "credentials.tokenLabel": "Belirteç",
  "credentials.httpBasicTitle": "HTTP Basic",
  "credentials.httpBasicDescription":
    "Yerel çalışma zamanı tarafından tam servis kaynağıyla sınırlandırılır.",
  "credentials.providerLoginTitle": "Sağlayıcı oturumu",
  "credentials.providerLoginDescription":
    "Desteklenen sağlayıcı bağdaştırıcıları için; hiçbir zaman sayfaya eklenmez.",
  "credentials.passwordLabel": "Parola",
  "credentials.usernameLabel": "Kullanıcı adı",
  "credentials.apiNone": "Yerel API bağdaştırıcısı yok",
  "credentials.apiHomarr": "Homarr API anahtarı",
  "credentials.apiGlancesBasic": "Glances HTTP Basic",
  "credentials.apiGlancesBearer": "Glances bearer belirteci",
  "credentials.browserProfile": "Kalıcı tarayıcı profili",
  "credentials.browserHttpBasic": "Tam kaynaklı HTTP Basic",

  "authentication.notConfigured": "Yapılandırılmamış",
  "authentication.notValidated": "Doğrulanmamış",
  "authentication.validating": "Doğrulanıyor…",
  "authentication.validated": "Doğrulandı",
  "authentication.rejected": "Reddedildi",
  "authentication.temporarilyUnavailable": "Geçici olarak kullanılamıyor",
  "authentication.waitingToRetry": "Yeniden denemek için bekleniyor",
  "authentication.missingCredential":
    "Doğrulamadan önce gerekli tüm kimlik bilgilerini kaydedin.",
  "authentication.endpointChanged":
    "Servis uç noktası değişti. Yeni kaynak için yeni kimlik bilgilerini kaydedin.",
  "authentication.unauthorized": "Servis kayıtlı kimlik bilgisini reddetti.",
  "authentication.forbidden":
    "Kimlik bilgisi kabul edildi ancak gerekli izne sahip değil.",
  "authentication.rateLimited":
    "Servis kimlik doğrulama kontrollerini hız sınırına tabi tutuyor.",
  "authentication.timeout": "Kimlik doğrulama kontrolü zaman aşımına uğradı.",
  "authentication.tls": "TLS doğrulaması kimlik doğrulama kontrolünü engelledi.",
  "authentication.connection":
    "Kimlik doğrulaması için servise ulaşılamadı.",
  "authentication.apiUnavailable":
    "Sağlayıcının kimlik doğrulama uç noktası kullanılamıyor.",
  "authentication.invalidData":
    "Sağlayıcı geçersiz bir kimlik doğrulama yanıtı döndürdü.",
  "authentication.insecureTransport":
    "Yerel HTTP'ye açıkça izin verilene kadar düz metin HTTP üzerinde otomatik kimlik bilgileri engellenir.",
  "authentication.vaultUnavailable":
    "Yerel kimlik bilgisi kasası geçici olarak kullanılamıyor.",
  "authentication.validationInProgress":
    "Bir kimlik doğrulama kontrolü zaten sürüyor.",
  "authentication.retryInSeconds":
    "{{reason}} Yaklaşık {{count}} saniye sonra yeniden deneyin.",
  "authentication.enableAdapter":
    "Otomatik kimlik doğrulamayı etkinleştirmek için izin verilen bir bağdaştırıcı seçin.",
  "authentication.storedReady":
    "Gerekli kimlik bilgileri kayıtlı ve doğrulanmaya hazır.",
  "authentication.notValidatedDescription":
    "Otomatik kimlik doğrulama henüz doğrulanmadı.",
  "authentication.validatingDescription":
    "Personal Hub kimlik bilgisini açığa çıkarmadan bağdaştırıcıyı kontrol ediyor.",
  "authentication.validDescription":
    "Yerel bağdaştırıcı kayıtlı kimlik bilgisini kabul etti.",
  "authentication.invalidDescription":
    "Otomatik kimlik doğrulama kontrolü başarısız oldu.",
  "authentication.unavailableDescription":
    "Bağdaştırıcı şu anda kontrolünü tamamlayamadı.",
  "authentication.backoffDescription":
    "Kimlik doğrulama kontrolleri geçici olarak duraklatıldı.",

  "maintenance.kicker": "Kurtarma",
  "maintenance.title": "Yapılandırma bakımı",
  "maintenance.bundledDefaults": "Paketle gelen varsayılanlar",
  "maintenance.bundledDefaultsDescription":
    "Kayıtlı kataloğu uygulamayla gelen sürümle değiştirin.",
  "maintenance.resetting": "Sıfırlanıyor…",
  "maintenance.resetDefaults": "Varsayılanları sıfırla",
  "maintenance.lastBackup": "Bilinen son yedek",
  "maintenance.lastBackupDescription":
    "En son kayıtlı değişiklikten önce oluşturulan yedeği geri yükleyin.",
  "maintenance.restoring": "Geri yükleniyor…",
  "maintenance.restoreBackup": "Yedeği geri yükle",

  "background.kicker": "Arka plan çalışma zamanı",
  "background.title": "Deneysel masaüstü kartları",
  "background.experimentBadge": "Katılımlı deney",
  "background.serverCardTitle": "Sunucu metrikleri",
  "background.serverCardDescription":
    "Glances'tan CPU, bellek, ağ, çalışma süresi ve yük bilgileri.",
  "background.storageCardTitle": "Depolama",
  "background.storageCardDescription":
    "Glances'tan sunucu birimi kullanımı ve kapasite uyarıları.",
  "background.servicesCardTitle": "Servis uyarıları",
  "background.servicesCardDescription":
    "Yapılandırılmış servis sağlık kontrollerinden çevrimdışı ve uyarı durumları.",
  "background.needsGlances": "Glances gerekli",
  "background.noHealthTargets": "Sağlık hedefi yok",
  "background.glancesUnavailableDescription":
    "Etkin bir Glances sağlayıcısı yapılandırılana kadar bu kart durur.",
  "background.healthUnavailableDescription":
    "En az bir etkin servis kontrol edilebilir olana kadar bu kart durur.",
  "background.updatedNotice": "Arka plan çalışma zamanı ayarları güncellendi.",
  "background.reading": "Arka plan çalışma zamanı tercihleri okunuyor.",
  "background.unavailable": "Arka plan çalışma zamanı tercihleri kullanılamıyor.",
  "background.allowCards": "Masaüstü kartlarına izin ver",
  "background.allowCardsDescription":
    "Varsayılan olarak kapalıdır. Kapalıyken Personal Hub kart penceresi, WebView veya kart yoklama işi oluşturmaz.",
  "background.enabledNotice": "Deneysel masaüstü kartları etkinleştirildi.",
  "background.disabledNotice":
    "Deneysel masaüstü kartları ve arka plan işleri kapatıldı.",
  "background.layerNote":
    "Kartlar desteklenen her zaman altta pencere katmanını kullanır. Bu deneyde Explorer masaüstü gömme özelliği etkin değildir.",
  "background.cardEnabledNotice": "{{card}} kartı etkinleştirildi.",
  "background.cardDisabledNotice": "{{card}} kartı devre dışı bırakıldı.",
  "background.keepInTray": "Personal Hub'ı bildirim alanında tut",
  "background.trayAvailableDescription":
    "Ana pencereyi kapatmak etkin kartları çalışır durumda tutar. Personal Hub'ı tamamen durdurmak için tepsi menüsündeki Çık seçeneğini kullanın.",
  "background.trayUnavailableDescription":
    "Bu Windows oturumunda bildirim alanı entegrasyonu kullanılamıyor; ana pencereyi kapatmak Personal Hub'dan güvenle çıkar.",
  "background.closeToTrayEnabled": "Tepsiye kapatma etkinleştirildi.",
  "background.closeQuits": "Ana pencere kapatıldığında Personal Hub'dan çıkılacak.",
  "background.runtimeOff":
    "Masaüstü kartları kapalı. Ana pencere kapatıldığında Personal Hub normal şekilde sonlanır.",
  "background.runtimeTrayUnavailableOne":
    "{{count}} masaüstü kartı kullanılabilir; ancak tepsi entegrasyonu olmadığından ana pencere kapatılınca Personal Hub sonlanır.",
  "background.runtimeTrayUnavailableOther":
    "{{count}} masaüstü kartı kullanılabilir; ancak tepsi entegrasyonu olmadığından ana pencere kapatılınca Personal Hub sonlanır.",
  "background.runtimeNoneAvailable":
    "Seçili kartların hiçbiri şu anda kullanılamıyor; pencereyi kapatmak boşta bir arka plan çalışma zamanı bırakmaz.",
  "background.runtimeAvailableOne":
    "{{count}} masaüstü kartı arka planda çalıştırılabilir.",
  "background.runtimeAvailableOther":
    "{{count}} masaüstü kartı arka planda çalıştırılabilir.",

  "widget.disableFailed": "Bu masaüstü kartı devre dışı bırakılamadı.",
  "widget.moveFailed": "Bu masaüstü kartı taşınamadı.",
  "widget.cardLabel": "{{title}} masaüstü kartı",
  "widget.refreshTitle": "{{title}} kartını yenile",
  "widget.turnOffTitle": "{{title}} kartını kapat",
  "widget.metricUsage": "{{metric}} kullanımı",
  "widget.temperature": "{{temperature}} sıcaklık",
  "widget.memoryUsage": "{{used}} / {{total}}",
  "widget.networkUpload": "{{upload}} yükleme",
  "widget.volumeCountOne": "{{count}} birim",
  "widget.volumeCountOther": "{{count}} birim",
  "widget.warningCountOne": "{{count}} uyarı",
  "widget.warningCountOther": "{{count}} uyarı",
  "widget.storageUsage": "{{name}} depolama kullanımı",
  "widget.usedOf": "{{total}} kapasitenin {{used}} kadarı",
  "widget.serviceNeedsAttentionOne": "{{count}} servis dikkat gerektiriyor",
  "widget.serviceNeedsAttentionOther": "{{count}} servis dikkat gerektiriyor",
  "widget.attentionPending": "Uyarı durumu bekleniyor",
  "widget.offlineCount": "{{count}} çevrimdışı",
  "widget.healthComplete": "Sağlık kontrolleri tamamlandı",
  "widget.checkingServices": "Sunucu servisleri kontrol ediliyor",
  "widget.serviceDataUnavailable": "Servis verisi kullanılamıyor",
  "widget.monitoringNotConfigured": "İzleme yapılandırılmamış",
  "widget.serverOnline": "Sunucu çevrimiçi",
  "widget.connecting": "Sunucuya bağlanılıyor",
  "widget.serverDataUnavailable": "Sunucu verisi kullanılamıyor",
  "widget.refresh": "Sunucu verisini yenile",
  "widget.turnOff": "Bu masaüstü kartını kapat",
  "widget.trendPending": "Eğilim bekleniyor",
  "widget.homeServer": "Ev Sunucusu",
  "widget.temperatureUnavailable": "Sıcaklık kullanılamıyor",
  "widget.down": "İndirme",
  "widget.up": "yükleme",
  "widget.serverUptimeAndLoad": "Sunucu çalışma süresi ve yükü",
  "widget.uptime": "Çalışma süresi",
  "widget.loadAverage": "Yük ortalaması",
  "widget.recentTrends": "Son sunucu eğilimleri",
  "widget.recentCpuTrend": "Son CPU kullanım eğilimi",
  "widget.recentMemoryTrend": "Son bellek kullanım eğilimi",
  "widget.recentNetworkTrend": "Son ağ indirme eğilimi",
  "widget.serviceOfflineDescription": "Servis çevrimdışı.",
  "widget.serviceWarningDescription": "Servis dikkat gerektiriyor.",
  "widget.warning": "Uyarı",
  "widget.serviceAttention": "Servis uyarıları",
  "widget.allServicesHealthy": "Tüm servisler sağlıklı",
  "widget.allServicesHealthyDescription": "Sunucu servisi uyarısı yok.",
  "widget.serviceStatusUnavailable": "Servis durumu kullanılamıyor",
  "widget.serviceStatusUnavailableDescription":
    "Sunucudan sağlık kontrolleri bekleniyor.",
  "widget.serverVolume": "Sunucu birimi",
  "widget.critical": "Kritik",
  "widget.lowSpace": "Düşük alan",
  "widget.serverStorage": "Sunucu depolaması",
  "widget.awaitingCapacity": "Kapasite bekleniyor",
  "widget.capacityHealthy": "Kapasite sağlıklı",
  "widget.noVolumeData": "Birim verisi yok",
  "widget.noVolumeDataDescription":
    "Sunucudan depolama metrikleri bekleniyor.",

  "appearance.kicker": "Görünüm",
  "appearance.title": "Tema ve dil",
  "appearance.description":
    "Personal Hub'ın nasıl görüneceğini ve hangi dili kullanacağını seçin.",
  "appearance.colorMode": "Renk modu",
  "appearance.colorModeSystem": "Sistem",
  "appearance.colorModeDark": "Koyu",
  "appearance.colorModeLight": "Açık",
  "appearance.colorModeSystemDescription":
    "Windows'u izler ve renk modu değiştiğinde güncellenir.",
  "appearance.colorModeDarkDescription": "Her zaman koyu paleti kullanır.",
  "appearance.colorModeLightDescription": "Her zaman açık paleti kullanır.",
  "appearance.theme": "Tema",
  "appearance.themeDefault": "Varsayılan",
  "appearance.themeCode": "Kod",
  "appearance.themeTranslucent": "Yarı saydam",
  "appearance.themeMinimal": "Minimal",
  "appearance.themeCustom": "Özel",
  "appearance.themeDefaultDescription":
    "Personal Hub'ın özgün görünümü ve boşluk düzeni.",
  "appearance.themeCodeDescription":
    "Monospace yazılı, ölçülü bir kod editörü paleti.",
  "appearance.themeTranslucentDescription":
    "Denetimli yarı saydamlığa sahip yumuşak katmanlı yüzeyler.",
  "appearance.themeMinimalDescription":
    "Daha az süsleme ve sıkı bir görsel ritim.",
  "appearance.themeCustomDescription":
    "Tema Stüdyosu'nda yalnız güvenli tokenlarla oluşturulan tema.",
  "appearance.language": "Dil",
  "appearance.languageSystem": "Sistem dili",
  "appearance.languageEnglish": "İngilizce",
  "appearance.languageTurkish": "Türkçe",
  "appearance.languageSystemDescription":
    "Windows'un tercih ettiği dili kullanır.",
  "appearance.languageEnglishDescription":
    "Personal Hub'ı İngilizce kullanır.",
  "appearance.languageTurkishDescription":
    "Personal Hub'ı Türkçe kullanır.",
  "appearance.preview": "Canlı önizleme",
  "appearance.loading": "Kayıtlı görünüm ayarları okunuyor…",
  "appearance.unsavedChanges": "Kaydedilmemiş görünüm değişiklikleri",
  "appearance.savedState": "Görünüm güncel",
  "appearance.savedNotice": "Görünüm tercihleri kaydedildi.",
  "appearance.cancelledNotice": "Canlı önizleme iptal edildi.",
  "appearance.recoveryNotice": "Görünüm kurtarma bildirimi",
  "appearance.errorTitle": "Görünüm işlemi başarısız oldu",
  "appearance.reset": "Görünümü sıfırla",
  "appearance.resetDescription":
    "Koyu Varsayılan temayı, sistem dilini ve boş özel tokenları geri yükler.",
  "appearance.resetConfirmation":
    "Kayıtlı görünüm güvenli varsayılanlara sıfırlansın mı?",
  "appearance.resetNotice": "Güvenli görünüm varsayılanları geri yüklendi.",
  "appearance.themeStudio": "Tema Stüdyosu",
  "appearance.themeStudioDescription":
    "Koyu ve açık mod için izinli renk ve sayısal tasarım tokenlarını düzenleyin.",
  "appearance.themeStudioSafety":
    "Tema Stüdyosu CSS, HTML, JavaScript, yazı tipi veya URL kabul etmez.",
  "appearance.studioMode": "Düzenlenen palet",
  "appearance.studioDark": "Koyu tokenlar",
  "appearance.studioLight": "Açık tokenlar",
  "appearance.studioColors": "Renkler",
  "appearance.studioMetrics": "Şekil, yoğunluk ve efektler",
  "appearance.studioOverride": "Özel değer",
  "appearance.studioInherited": "Tema değeri",
  "appearance.studioResetToken": "Tema değerini geri yükle",
  "appearance.studioUseCustom": "Özel tema olarak kullan",
  "appearance.studioCustomReady":
    "Geçerli tokenlar özel tema olarak hazır. Kalıcı olması için kaydedin.",
  "appearance.studioInvalidValue":
    "Bu değer temayı geçersiz kılacağı için uygulanmadı.",
  "appearance.colorValueHint": "Hex renk (#RRGGBB veya #RRGGBBAA)",
  "appearance.tokenBackground": "Arka plan",
  "appearance.tokenSurface": "Yüzey",
  "appearance.tokenSurfaceElevated": "Yükseltilmiş yüzey",
  "appearance.tokenSurfaceHover": "Üzerine gelme yüzeyi",
  "appearance.tokenBorder": "Kenarlık",
  "appearance.tokenBorderStrong": "Güçlü kenarlık",
  "appearance.tokenTextPrimary": "Birincil metin",
  "appearance.tokenTextSecondary": "İkincil metin",
  "appearance.tokenTextTertiary": "Üçüncül metin",
  "appearance.tokenAccent": "Vurgu",
  "appearance.tokenAccentHover": "Vurgu üzerine gelme",
  "appearance.tokenOnline": "Çevrimiçi",
  "appearance.tokenOffline": "Çevrimdışı",
  "appearance.tokenWarning": "Uyarı",
  "appearance.tokenRadiusSmall": "Küçük yarıçap",
  "appearance.tokenRadiusMedium": "Orta yarıçap",
  "appearance.tokenRadiusLarge": "Büyük yarıçap",
  "appearance.tokenDensity": "Yoğunluk",
  "appearance.tokenTypeScale": "Yazı ölçeği",
  "appearance.tokenShadow": "Gölge gücü",
  "appearance.tokenTranslucency": "Yarı saydamlık",
  "appearance.tokenBlur": "Bulanıklık",
  "appearance.importTheme": "Tema içe aktar",
  "appearance.importDescription":
    "Bir Personal Hub görünüm JSON dosyası yapıştırın veya seçin. Önizlemeden önce katı biçimde doğrulanır.",
  "appearance.importPasteLabel": "Görünüm JSON'u",
  "appearance.importPlaceholder": "Bir Personal Hub görünüm paketi yapıştırın…",
  "appearance.chooseFile": "JSON dosyası seç",
  "appearance.applyImport": "İçe aktarılan temayı önizle",
  "appearance.importReady":
    "İçe aktarılan görünüm önizlemeye hazır. Kalıcı olması için kaydedin.",
  "appearance.importFailed": "Görünüm paketi içe aktarılamadı.",
  "appearance.fileTooLarge": "Seçilen dosya 64 KiB sınırından büyük.",
  "appearance.exportTheme": "Tema dışa aktar",
  "appearance.exportDescription":
    "Son kaydedilen görünümü güvenli ve bildirimsel JSON olarak dışa aktarır.",
  "appearance.copyJson": "JSON'u kopyala",
  "appearance.downloadJson": "JSON'u indir",
  "appearance.copiedJson": "Görünüm JSON'u kopyalandı.",
  "appearance.downloadedJson": "Görünüm JSON'u indirme işlemi başladı.",
  "appearance.clipboardUnavailable": "Pano erişimi kullanılamıyor.",
  "appearance.exportFailed": "Kayıtlı görünüm dışa aktarılamadı.",
} as const satisfies PartialTranslationCatalog;


export const turkishCatalog: TranslationCatalog = {
  ...englishCatalog,
  ...turkishOverrides,
};
