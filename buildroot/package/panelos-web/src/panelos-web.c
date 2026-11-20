/*
 * panelos-web: the Home Assistant frontend in WPE WebKit for the PanelOS app.
 *
 * Runs inside the web bundle (chroot). It renders one web view off screen
 * on the GPU. While the web view is all there is on the screen ("direct"),
 * every finished frame is drawn on the GPU into one of the app's display
 * buffers (dma-buf) and the app flips the display to it. Otherwise frames
 * are read back into shared memory and the app draws them like an image
 * with its own layers on top.
 * Touch input, the page to load and the login token come from the app.
 *
 * Protocol on the socket (file descriptor from --fd), one line per message:
 *   app -> web   LOAD <url>
 *                KIOSK <hide_header 0|1> <hide_sidebar 0|1>   (before LOAD)
 *                TOKEN <access_token> <expires_in>
 *                TOUCH <d|m|u> <x> <y>
 *                VISIBLE <0|1>
 *                RELOAD
 *                BUFS <n> <w> <h> <pitch>  (with n dma-bufs, SCM_RIGHTS: the app's
 *                                        XRGB8888 display buffers)
 *                SKIP <path> ...        (resources the page must not load, e.g. card
 *                                        resources the dashboard does not use; none:
 *                                        load everything; reloads the page)
 *                ZOOM <percent>         (page size; 100 = as in a browser)
 *                DIRECT <0|1>           (frames into the display buffers or not)
 *                FREE <i>               (display buffer i is off the screen again)
 *                ATTOP <x> <y>          (is what lies under this screen point scrolled
 *                                        to its top? for pull to refresh)
 *                SCROLLFIND, SCROLLTEST <px> <ms>   (measurements: find the page's
 *                                        largest scrolling element, then scroll it
 *                                        by a script, without touches)
 *   web -> app   SHM <width> <height>   (with the memfd, SCM_RIGHTS; two
 *                                        frames of width*height*4 bytes RGBA)
 *                FRAME <n> <t> <readback>  (frame n is in slot n % 2; t = when
 *                                        WebKit handed it over, CLOCK_MONOTONIC µs;
 *                                        readback = µs spent reading it back)
 *                FRAMEBUF <i> <t> <render>  (a direct frame is in display buffer i;
 *                                        render = µs spent drawing and waiting)
 *                SCROLL <result>        (answer to SCROLLFIND / SCROLLTEST)
 *                ATTOP <0|1>            (answer to ATTOP)
 *                AUTH                   (the page wants a fresh token)
 *                LOADED <percent>
 *                LOADING                (a page started loading: the app covers it)
 *                READY <reason>         (it has loaded and settled: show it)
 *                FAILED <reason>
 * Login: Home Assistant's external authentication, as in its mobile apps
 * (the page is opened with external_auth=1 and asks for tokens through
 * window.webkit.messageHandlers). The token goes only to a page on the
 * origin (scheme, host, port) of the page the app loaded: the view never
 * shows a document of another origin and opens no windows. Frames inside
 * the page and what it loads (pictures, card scripts) are not limited.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <GLES2/gl2ext.h>
#include <gbm.h>
#include <wayland-server.h>
#include <wpe/fdo-egl.h>
#include <wpe/fdo.h>
#include <wpe/webkit.h>

static struct {
    int sock;
    int width, height;
    const char *data_dir;
    uint8_t *shm;
    size_t frame_bytes;
    uint64_t frame_no;
    EGLDisplay dpy;
    EGLContext ctx;
    GLuint tex, fbo;
    PFNGLEGLIMAGETARGETTEXTURE2DOESPROC image_target_texture;
    struct wpe_view_backend_exportable_fdo *exportable;
    struct wpe_view_backend *backend;
    WebKitWebView *view;
    WebKitUserContentManager *content;
    int hide_header, hide_sidebar;
    char *origin;  /* origin of the page the app loaded (LOAD): the token goes only there */
    GPtrArray *auth_callbacks;  /* pending externalAuthSetToken names */
    GString *line;
    int touching;
    /* Direct frames: the app's display buffers. */
    int fds[8];
    int nfds;
    struct {
        EGLImageKHR image;
        GLuint tex, fbo;
        int busy;
    } bufs[4];
    int nbufs;
    int direct;
    int last_buf;
    GLuint prog;
    GLint pos_attr;
    struct wpe_fdo_egl_exported_image *waiting;  /* direct frame waiting for a free buffer */
    PFNEGLCREATEIMAGEKHRPROC create_image;
    WebKitUserContentFilterStore *filters;
    char *skip;  /* the last SKIP list, to skip saving the same filter again */
    int filter_saving;  /* LOAD waits for the filter (no double load) */
    char *waiting_uri;
    /* Is the page ready to be shown? (see check_ready) */
    int loading, ready_check_running, stable;
    uint32_t load_started, load_finished, last_frame, root_seen;
    long prev_count;
} S = { .sock = -1, .width = 480, .height = 480, .data_dir = "/data", .last_buf = -1 };

static void send_line(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
static void send_line(const char *fmt, ...)
{
    char buf[512];
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, sizeof buf - 1, fmt, ap);
    va_end(ap);
    if (n < 0)
        return;
    if (n > (int)sizeof buf - 2)
        n = sizeof buf - 2;
    buf[n++] = '\n';
    if (write(S.sock, buf, n) < 0 && errno != EAGAIN)
        exit(0);  /* the app is gone */
}

static uint32_t now_ms(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000u + ts.tv_nsec / 1000000u;
}

/* ---------- frames ---------- */

static uint8_t *next_slot(void)
{
    return S.shm + ((S.frame_no + 1) % 2) * S.frame_bytes;
}

static uint64_t now_us(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000u + ts.tv_nsec / 1000u;
}

static void publish_frame(uint64_t produced, uint64_t readback)
{
    S.frame_no++;
    send_line("FRAME %llu %llu %llu", (unsigned long long)S.frame_no, (unsigned long long)produced,
              (unsigned long long)readback);
}

static int free_buf(void)
{
    for (int i = 0; i < S.nbufs; i++)
        if (!S.bufs[i].busy)
            return i;
    return -1;
}

/* Draws WebKit's frame into display buffer i on the GPU and waits for it. */
static void draw_direct(int i, struct wpe_fdo_egl_exported_image *image)
{
    uint64_t produced = now_us();
    glBindTexture(GL_TEXTURE_2D, S.tex);
    S.image_target_texture(GL_TEXTURE_2D, wpe_fdo_egl_exported_image_get_egl_image(image));
    glBindFramebuffer(GL_FRAMEBUFFER, S.bufs[i].fbo);
    glViewport(0, 0, S.width, S.height);
    glUseProgram(S.prog);
    static const GLfloat quad[] = { -1, -1, 1, -1, -1, 1, 1, 1 };
    glVertexAttribPointer(S.pos_attr, 2, GL_FLOAT, GL_FALSE, 0, quad);
    glEnableVertexAttribArray(S.pos_attr);
    glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
    glFinish();
    S.bufs[i].busy = 1;
    S.last_buf = i;
    send_line("FRAMEBUF %d %llu %llu", i, (unsigned long long)produced,
              (unsigned long long)(now_us() - produced));
}

static void done_with(struct wpe_fdo_egl_exported_image *image)
{
    wpe_view_backend_exportable_fdo_egl_dispatch_release_exported_image(S.exportable, image);
    wpe_view_backend_exportable_fdo_dispatch_frame_complete(S.exportable);
}

static void on_egl_image(void *data, struct wpe_fdo_egl_exported_image *image)
{
    (void)data;
    uint64_t produced = now_us();
    S.last_frame = now_ms();
    int w = wpe_fdo_egl_exported_image_get_width(image);
    int h = wpe_fdo_egl_exported_image_get_height(image);
    if (S.direct && S.nbufs > 0 && w == S.width && h == S.height) {
        int i = free_buf();
        if (i < 0) {
            /* Wait for the app to give a buffer back (FREE): WebKit holds
             * the next frame meanwhile. */
            if (S.waiting)
                done_with(S.waiting);
            S.waiting = image;
            return;
        }
        draw_direct(i, image);
        done_with(image);
        return;
    }
    if (w == S.width && h == S.height) {
        glBindTexture(GL_TEXTURE_2D, S.tex);
        S.image_target_texture(GL_TEXTURE_2D, wpe_fdo_egl_exported_image_get_egl_image(image));
        glBindFramebuffer(GL_FRAMEBUFFER, S.fbo);
        glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, S.tex, 0);
        uint8_t *dst = next_slot();
        /* WebKit's image is top row first, as the app draws it. */
        glReadPixels(0, 0, w, h, GL_RGBA, GL_UNSIGNED_BYTE, dst);
        publish_frame(produced, now_us() - produced);
    }
    wpe_view_backend_exportable_fdo_egl_dispatch_release_exported_image(S.exportable, image);
    wpe_view_backend_exportable_fdo_dispatch_frame_complete(S.exportable);
}

static void on_shm_buffer(void *data, struct wpe_fdo_shm_exported_buffer *buffer)
{
    (void)data;
    uint64_t produced = now_us();
    S.last_frame = now_ms();
    struct wl_shm_buffer *shm = wpe_fdo_shm_exported_buffer_get_shm_buffer(buffer);
    int w = wl_shm_buffer_get_width(shm), h = wl_shm_buffer_get_height(shm);
    int stride = wl_shm_buffer_get_stride(shm);
    uint32_t format = wl_shm_buffer_get_format(shm);
    if (w == S.width && h == S.height &&
        (format == WL_SHM_FORMAT_ARGB8888 || format == WL_SHM_FORMAT_XRGB8888)) {
        uint8_t *dst = next_slot();
        wl_shm_buffer_begin_access(shm);
        const uint8_t *src = wl_shm_buffer_get_data(shm);
        for (int y = 0; y < h; y++) {
            const uint8_t *s = src + (size_t)y * stride;
            uint8_t *d = dst + (size_t)y * w * 4;
            for (int x = 0; x < w; x++, s += 4, d += 4) {  /* BGRA -> RGBA */
                d[0] = s[2]; d[1] = s[1]; d[2] = s[0]; d[3] = 255;
            }
        }
        wl_shm_buffer_end_access(shm);
        publish_frame(produced, now_us() - produced);
    }
    wpe_view_backend_exportable_fdo_egl_dispatch_release_shm_exported_buffer(S.exportable, buffer);
    wpe_view_backend_exportable_fdo_dispatch_frame_complete(S.exportable);
}

static int init_gl(void)
{
    int fd = open("/dev/dri/renderD128", O_RDWR | O_CLOEXEC);
    if (fd < 0) {
        perror("panelos-web: /dev/dri/renderD128");
        return -1;
    }
    struct gbm_device *gbm = gbm_create_device(fd);
    PFNEGLGETPLATFORMDISPLAYEXTPROC get_display =
        (PFNEGLGETPLATFORMDISPLAYEXTPROC)eglGetProcAddress("eglGetPlatformDisplayEXT");
    S.dpy = get_display ? get_display(EGL_PLATFORM_GBM_KHR, gbm, NULL) : EGL_NO_DISPLAY;
    if (S.dpy == EGL_NO_DISPLAY || !eglInitialize(S.dpy, NULL, NULL)) {
        fprintf(stderr, "panelos-web: no EGL display\n");
        return -1;
    }
    eglBindAPI(EGL_OPENGL_ES_API);
    static const EGLint cfg_attr[] = { EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT, EGL_SURFACE_TYPE, 0,
                                       EGL_NONE };
    EGLConfig cfg;
    EGLint n = 0;
    if (!eglChooseConfig(S.dpy, cfg_attr, &cfg, 1, &n) || n < 1) {
        fprintf(stderr, "panelos-web: no EGL config\n");
        return -1;
    }
    static const EGLint ctx_attr[] = { EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE };
    S.ctx = eglCreateContext(S.dpy, cfg, EGL_NO_CONTEXT, ctx_attr);
    if (S.ctx == EGL_NO_CONTEXT || !eglMakeCurrent(S.dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, S.ctx)) {
        fprintf(stderr, "panelos-web: no EGL context\n");
        return -1;
    }
    S.image_target_texture =
        (PFNGLEGLIMAGETARGETTEXTURE2DOESPROC)eglGetProcAddress("glEGLImageTargetTexture2DOES");
    glGenTextures(1, &S.tex);
    glBindTexture(GL_TEXTURE_2D, S.tex);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
    glGenFramebuffers(1, &S.fbo);
    return wpe_fdo_initialize_for_egl_display(S.dpy) ? 0 : -1;
}

static GLuint compile(GLenum type, const char *src)
{
    GLuint sh = glCreateShader(type);
    glShaderSource(sh, 1, &src, NULL);
    glCompileShader(sh);
    GLint ok = 0;
    glGetShaderiv(sh, GL_COMPILE_STATUS, &ok);
    if (!ok) {
        char log[512];
        glGetShaderInfoLog(sh, sizeof log, NULL, log);
        fprintf(stderr, "panelos-web: shader: %s\n", log);
    }
    return sh;
}

/* A quad that copies WebKit's frame (top row first) into a display buffer. */
static int init_blit(void)
{
    static const char vs[] =
        "attribute vec2 pos;\n"
        "varying vec2 uv;\n"
        "void main() { uv = (pos + 1.0) * 0.5; gl_Position = vec4(pos, 0.0, 1.0); }\n";
    static const char fs[] =
        "precision mediump float;\n"
        "varying vec2 uv;\n"
        "uniform sampler2D frame;\n"
        "void main() { gl_FragColor = vec4(texture2D(frame, uv).rgb, 1.0); }\n";
    S.prog = glCreateProgram();
    glAttachShader(S.prog, compile(GL_VERTEX_SHADER, vs));
    glAttachShader(S.prog, compile(GL_FRAGMENT_SHADER, fs));
    glLinkProgram(S.prog);
    GLint ok = 0;
    glGetProgramiv(S.prog, GL_LINK_STATUS, &ok);
    S.pos_attr = glGetAttribLocation(S.prog, "pos");
    S.create_image = (PFNEGLCREATEIMAGEKHRPROC)eglGetProcAddress("eglCreateImageKHR");
    return ok && S.create_image ? 0 : -1;
}

/* The app's display buffers (BUFS): each one a texture we can render into. */
static void import_bufs(int n, int w, int h, int pitch)
{
    if (n > 4 || n > S.nfds || w != S.width || h != S.height || !S.prog) {
        fprintf(stderr, "panelos-web: display buffers not usable (%d of %d fds, %dx%d)\n", n, S.nfds, w, h);
        return;
    }
    for (int i = 0; i < n; i++) {
        const EGLint attr[] = {
            EGL_WIDTH, w, EGL_HEIGHT, h,
            EGL_LINUX_DRM_FOURCC_EXT, 0x34325258, /* DRM_FORMAT_XRGB8888 */
            EGL_DMA_BUF_PLANE0_FD_EXT, S.fds[i],
            EGL_DMA_BUF_PLANE0_OFFSET_EXT, 0,
            EGL_DMA_BUF_PLANE0_PITCH_EXT, pitch,
            EGL_NONE,
        };
        EGLImageKHR image = S.create_image(S.dpy, EGL_NO_CONTEXT, EGL_LINUX_DMA_BUF_EXT, NULL, attr);
        close(S.fds[i]);
        if (image == EGL_NO_IMAGE_KHR) {
            fprintf(stderr, "panelos-web: display buffer %d: no EGL image (0x%x)\n", i, eglGetError());
            S.nbufs = 0;
            return;
        }
        S.bufs[i].image = image;
        glGenTextures(1, &S.bufs[i].tex);
        glBindTexture(GL_TEXTURE_2D, S.bufs[i].tex);
        S.image_target_texture(GL_TEXTURE_2D, image);
        glGenFramebuffers(1, &S.bufs[i].fbo);
        glBindFramebuffer(GL_FRAMEBUFFER, S.bufs[i].fbo);
        glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, S.bufs[i].tex, 0);
        if (glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE) {
            fprintf(stderr, "panelos-web: display buffer %d cannot be rendered into\n", i);
            S.nbufs = 0;
            return;
        }
        S.bufs[i].busy = 0;
    }
    for (int i = n; i < S.nfds; i++)
        close(S.fds[i]);
    S.nfds = 0;
    S.nbufs = n;
    fprintf(stderr, "panelos-web: %d display buffers for direct frames\n", n);
}

/* Leaving direct mode: the app needs the frame on screen as an image. */
static void last_frame_to_shm(void)
{
    if (S.last_buf < 0)
        return;
    uint64_t produced = now_us();
    glBindFramebuffer(GL_FRAMEBUFFER, S.bufs[S.last_buf].fbo);
    uint8_t *dst = next_slot();
    glReadPixels(0, 0, S.width, S.height, GL_RGBA, GL_UNSIGNED_BYTE, dst);
    publish_frame(produced, now_us() - produced);
}

static int init_shm(void)
{
    S.frame_bytes = (size_t)S.width * S.height * 4;
    int fd = memfd_create("panelos-web", MFD_CLOEXEC);
    if (fd < 0 || ftruncate(fd, 2 * S.frame_bytes) < 0)
        return -1;
    S.shm = mmap(NULL, 2 * S.frame_bytes, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (S.shm == MAP_FAILED)
        return -1;
    char text[64];
    int len = snprintf(text, sizeof text, "SHM %d %d\n", S.width, S.height);
    struct iovec iov = { text, len };
    char ctl[CMSG_SPACE(sizeof(int))];
    struct msghdr msg = { .msg_iov = &iov, .msg_iovlen = 1, .msg_control = ctl, .msg_controllen = sizeof ctl };
    struct cmsghdr *c = CMSG_FIRSTHDR(&msg);
    c->cmsg_level = SOL_SOCKET;
    c->cmsg_type = SCM_RIGHTS;
    c->cmsg_len = CMSG_LEN(sizeof(int));
    memcpy(CMSG_DATA(c), &fd, sizeof(int));
    if (sendmsg(S.sock, &msg, 0) < 0)
        return -1;
    close(fd);
    return 0;
}

/* ---------- Home Assistant: login and kiosk look ---------- */

static void run_js(const char *script)
{
    webkit_web_view_evaluate_javascript(S.view, script, -1, NULL, NULL, NULL, NULL, NULL);
}

/* Names and tokens go into JavaScript: only the characters they consist of. */
static gboolean plain(const char *s)
{
    if (!s || !*s)
        return FALSE;
    for (; *s; s++)
        if (!g_ascii_isalnum(*s) && *s != '_' && *s != '-' && *s != '.')
            return FALSE;
    return TRUE;
}

/* Callback names: one JavaScript name, no path through other objects. */
static gboolean identifier(const char *s)
{
    if (!s || !*s || g_ascii_isdigit(*s))
        return FALSE;
    for (; *s; s++)
        if (!g_ascii_isalnum(*s) && *s != '_')
            return FALSE;
    return TRUE;
}

/* The origin of a URI as WebKit writes it ("http://host:8123"; location.origin
 * in the page reads the same), NULL for none (an opaque origin). */
static char *origin_of(const char *uri)
{
    if (!uri || !*uri)
        return NULL;
    WebKitSecurityOrigin *o = webkit_security_origin_new_for_uri(uri);
    char *s = webkit_security_origin_to_string(o);
    webkit_security_origin_unref(o);
    return s;
}

/* Is the URI on the origin of the page the app loaded? */
static gboolean own_origin(const char *uri)
{
    char *o = origin_of(uri);
    gboolean same = o && S.origin && strcmp(o, S.origin) == 0;
    g_free(o);
    return same;
}

/* The page the app loads: its origin is the only one that gets the token
 * (it goes into JavaScript: only the characters an origin consists of). */
static void set_origin(const char *uri)
{
    g_free(S.origin);
    S.origin = origin_of(uri);
    for (const char *c = S.origin; c && *c; c++) {
        if (!g_ascii_isalnum(*c) && !strchr(".:/[]_-", *c)) {
            g_clear_pointer(&S.origin, g_free);
            break;
        }
    }
    if (!S.origin)
        fprintf(stderr, "panelos-web: the page has no origin to log in to\n");
}

static void on_get_auth(WebKitUserContentManager *m, JSCValue *value, gpointer data)
{
    (void)m; (void)data;
    char *callback = NULL;
    if (jsc_value_is_object(value)) {
        JSCValue *cb = jsc_value_object_get_property(value, "callback");
        if (jsc_value_is_string(cb))
            callback = jsc_value_to_string(cb);
        g_object_unref(cb);
    }
    if (!identifier(callback)) {
        g_free(callback);
        return;
    }
    /* Only Home Assistant's page asks (the answer goes to the page itself,
     * never into a frame inside it; answer_auth checks it again there). */
    if (!own_origin(webkit_web_view_get_uri(S.view))) {
        fprintf(stderr, "panelos-web: no token for a page of another site\n");
        g_free(callback);
        return;
    }
    g_ptr_array_add(S.auth_callbacks, callback);
    send_line("AUTH");
}

static void on_revoke_auth(WebKitUserContentManager *m, JSCValue *value, gpointer data)
{
    (void)m; (void)data;
    /* The panel keeps its login; the page just forgets its token. */
    if (!jsc_value_is_object(value))
        return;
    JSCValue *cb = jsc_value_object_get_property(value, "callback");
    char *callback = jsc_value_is_string(cb) ? jsc_value_to_string(cb) : NULL;
    g_object_unref(cb);
    if (identifier(callback)) {
        char *js = g_strdup_printf("window.%s && window.%s(true);", callback, callback);
        run_js(js);
        g_free(js);
    }
    g_free(callback);
}

static void on_external_bus(WebKitUserContentManager *m, JSCValue *value, gpointer data)
{
    (void)m; (void)data;
    if (!jsc_value_is_object(value))
        return;
    JSCValue *id = jsc_value_object_get_property(value, "id");
    JSCValue *type = jsc_value_object_get_property(value, "type");
    char *t = jsc_value_is_string(type) ? jsc_value_to_string(type) : NULL;
    if (t && strcmp(t, "config/get") == 0 && jsc_value_is_number(id)) {
        /* Nothing the page would offer through the app: no settings screen,
           no Assist button (the panel has its own voice satellite). */
        char *js = g_strdup_printf(
            "window.externalBus && window.externalBus({id: %d, type: 'result', success: true, result: "
            "{hasSettingsScreen: false, hasSidebar: false, canWriteTag: false, hasExoPlayer: false, "
            "canCommissionMatter: false, canImportThreadCredentials: false, hasAssist: false, "
            "hasBarCodeScanner: 0, canSetupImprov: false}});",
            jsc_value_to_int32(id));
        run_js(js);
        g_free(js);
    }
    g_free(t);
    g_object_unref(id);
    g_object_unref(type);
}

/* Header and sidebar: Home Assistant has no setting for a kiosk, so a style
   goes into the dashboard's shadow root (hui-root), and the sidebar is set
   to "always hidden" as the profile setting does it. */
static const char KIOSK_JS[] =
    "(() => {\n"
    "  const hideHeader = %d, hideSidebar = %d;\n"
    "  if (hideSidebar) { try { localStorage.setItem('dockedSidebar', JSON.stringify('always_hidden')); } catch (e) {} }\n"
    "  if (!hideHeader && !hideSidebar) return;\n"
    "  let css = '';\n"
    "  if (hideHeader) css += '.header, .toolbar { display: none !important; } #view { padding-top: 0 !important; min-height: 100vh !important; } :host { --header-height: 0px !important; }';\n"
    "  if (hideSidebar) css += ' ha-menu-button { display: none !important; }';\n"
    "  const apply = () => {\n"
    "    const main = document.querySelector('home-assistant')?.shadowRoot?.querySelector('home-assistant-main')?.shadowRoot;\n"
    "    const root = main?.querySelector('ha-panel-lovelace')?.shadowRoot?.querySelector('hui-root')?.shadowRoot;\n"
    "    if (root && !root.getElementById('panelos-kiosk')) {\n"
    "      const s = document.createElement('style'); s.id = 'panelos-kiosk'; s.textContent = css; root.appendChild(s);\n"
    "    }\n"
    "  };\n"
    "  setInterval(apply, 500);\n"
    "})();\n";

static void set_kiosk_script(void)
{
    webkit_user_content_manager_remove_all_scripts(S.content);
    char *js = g_strdup_printf(KIOSK_JS, S.hide_header, S.hide_sidebar);
    WebKitUserScript *script = webkit_user_script_new(js, WEBKIT_USER_CONTENT_INJECT_TOP_FRAME,
                                                      WEBKIT_USER_SCRIPT_INJECT_AT_DOCUMENT_START, NULL, NULL);
    webkit_user_content_manager_add_script(S.content, script);
    webkit_user_script_unref(script);
    g_free(js);
}

static void answer_auth(const char *token, int expires)
{
    if (!plain(token))
        return;
    if (!S.origin) {
        g_ptr_array_set_size(S.auth_callbacks, 0);
        return;
    }
    for (guint i = 0; i < S.auth_callbacks->len; i++) {
        const char *cb = g_ptr_array_index(S.auth_callbacks, i);
        /* Checked in the document that runs it: the page may have changed
         * since it asked. */
        char *js = g_strdup_printf("location.origin === '%s' && window.%s && "
                                   "window.%s(true, {access_token: '%s', expires_in: %d});",
                                   S.origin, cb, cb, token, expires);
        run_js(js);
        g_free(js);
    }
    g_ptr_array_set_size(S.auth_callbacks, 0);
}

/* ---------- input ---------- */

static void touch(char kind, int x, int y)
{
    static struct wpe_input_touch_event_raw point;
    enum wpe_input_touch_event_type type =
        kind == 'd' ? wpe_input_touch_event_type_down : kind == 'm' ? wpe_input_touch_event_type_motion
                                                                    : wpe_input_touch_event_type_up;
    if (type == wpe_input_touch_event_type_motion && !S.touching)
        return;
    point = (struct wpe_input_touch_event_raw){ .type = type, .time = now_ms(), .id = 0, .x = x, .y = y };
    struct wpe_input_touch_event ev = {
        .touchpoints = &point, .touchpoints_length = 1, .type = type, .id = 0, .time = point.time,
    };
    wpe_view_backend_dispatch_touch_event(S.backend, &ev);
    S.touching = type != wpe_input_touch_event_type_up;
}

static void set_visible(int on)
{
    const uint32_t states =
        wpe_view_activity_state_visible | wpe_view_activity_state_focused | wpe_view_activity_state_in_window;
    if (on)
        wpe_view_backend_add_activity_state(S.backend, states);
    else
        wpe_view_backend_remove_activity_state(S.backend, states);
}

/* ---------- measurements: scroll by a script ---------- */

/* The page's largest scrolling element (shadow roots included). */
static const char SCROLL_FIND_JS[] =
    "(() => {"
    "  const list = [];"
    "  const walk = (root) => {"
    "    for (const el of root.querySelectorAll('*')) {"
    "      if (el.clientHeight > 100 && el.scrollHeight > el.clientHeight + 40) {"
    "        const oy = getComputedStyle(el).overflowY;"
    "        if (oy === 'auto' || oy === 'scroll') list.push(el);"
    "      }"
    "      if (el.shadowRoot) walk(el.shadowRoot);"
    "    }"
    "  };"
    "  walk(document);"
    "  const se = document.scrollingElement;"
    "  if (se && se.scrollHeight > se.clientHeight + 40) list.push(se);"
    "  list.sort((a, b) => b.clientWidth * b.clientHeight - a.clientWidth * a.clientHeight);"
    "  const t = list[0] || null;"
    "  window.__panelosScroll = t;"
    "  return t ? t.tagName.toLowerCase() + ' ' + t.scrollHeight + '/' + t.clientHeight : 'none';"
    "})()";

/* Scrolls it by px over ms, one step per animation frame. */
static const char SCROLL_TEST_JS[] =
    "((px, ms) => {"
    "  const t = window.__panelosScroll;"
    "  if (!t) return 'none';"
    "  const y0 = t.scrollTop, start = performance.now();"
    "  const step = (now) => {"
    "    const k = Math.min(1, (now - start) / ms);"
    "    t.scrollTop = y0 + px * k;"
    "    if (k < 1) requestAnimationFrame(step);"
    "  };"
    "  requestAnimationFrame(step);"
    "  return 'from ' + y0;"
    "})(%d, %d)";

static void on_script_done(GObject *object, GAsyncResult *result, gpointer data)
{
    (void)data;
    GError *err = NULL;
    JSCValue *value = webkit_web_view_evaluate_javascript_finish(WEBKIT_WEB_VIEW(object), result, &err);
    if (value) {
        char *text = jsc_value_to_string(value);
        send_line("SCROLL %s", text);
        g_free(text);
        g_object_unref(value);
    } else {
        send_line("SCROLL error %s", err ? err->message : "?");
        g_clear_error(&err);
    }
}

static void run_script(const char *js)
{
    webkit_web_view_evaluate_javascript(S.view, js, -1, NULL, NULL, NULL, on_script_done, NULL);
}

/* ---------- pull to refresh: is the content under the finger at its top? ---------- */

/* The nearest scrolling element under a point (CSS px), through shadow roots
 * and slots, or else the page: 1 when it is scrolled to its top. */
static const char AT_TOP_JS[] =
    "((x, y) => {"
    "  let el = document.elementFromPoint(x, y);"
    "  while (el && el.shadowRoot) {"
    "    const inner = el.shadowRoot.elementFromPoint(x, y);"
    "    if (!inner || inner === el) break;"
    "    el = inner;"
    "  }"
    "  for (let n = el; n; ) {"
    "    if (n.nodeType === 1 && n.scrollHeight > n.clientHeight + 1) {"
    "      const oy = getComputedStyle(n).overflowY;"
    "      if (oy === 'auto' || oy === 'scroll') return n.scrollTop <= 0 ? 1 : 0;"
    "    }"
    "    n = n.assignedSlot || n.parentNode;"
    "    if (n && n.nodeType === 11) n = n.host;"
    "  }"
    "  const se = document.scrollingElement;"
    "  return !se || se.scrollTop <= 0 ? 1 : 0;"
    "})(%d, %d)";

static void on_at_top(GObject *object, GAsyncResult *result, gpointer data)
{
    (void)data;
    GError *err = NULL;
    JSCValue *value = webkit_web_view_evaluate_javascript_finish(WEBKIT_WEB_VIEW(object), result, &err);
    /* A page the script cannot look into (an error page) may be refreshed. */
    int top = 1;
    if (value) {
        top = jsc_value_to_int32(value) != 0;
        g_object_unref(value);
    } else {
        g_clear_error(&err);
    }
    send_line("ATTOP %d", top);
}

static void check_top(int x, int y)
{
    double zoom = webkit_web_view_get_zoom_level(S.view);
    if (zoom <= 0)
        zoom = 1;
    char *js = g_strdup_printf(AT_TOP_JS, (int)(x / zoom), (int)(y / zoom));
    webkit_web_view_evaluate_javascript(S.view, js, -1, NULL, NULL, NULL, on_at_top, NULL);
    g_free(js);
}

/* ---------- resources the page does not load ---------- */

/* The page LOAD waited for, or a reload with the new filter. */
static void after_filter(void)
{
    S.filter_saving = 0;
    if (S.waiting_uri) {
        webkit_web_view_load_uri(S.view, S.waiting_uri);
        g_clear_pointer(&S.waiting_uri, g_free);
    } else if (webkit_web_view_get_uri(S.view)) {
        webkit_web_view_reload(S.view);
    }
}

static void on_filter_saved(GObject *object, GAsyncResult *result, gpointer data)
{
    (void)data;
    GError *err = NULL;
    WebKitUserContentFilter *filter =
        webkit_user_content_filter_store_save_finish(WEBKIT_USER_CONTENT_FILTER_STORE(object), result, &err);
    if (!filter) {
        fprintf(stderr, "panelos-web: resource filter: %s\n", err ? err->message : "?");
        g_clear_error(&err);
        after_filter();
        return;
    }
    webkit_user_content_manager_remove_all_filters(S.content);
    webkit_user_content_manager_add_filter(S.content, filter);
    webkit_user_content_filter_unref(filter);
    after_filter();
}

/* SKIP: a content blocker rule per path (the path anywhere in the URL). */
static void set_skip(const char *list)
{
    if (S.skip && strcmp(S.skip, list) == 0)
        return;
    g_free(S.skip);
    S.skip = g_strdup(list);
    char **paths = g_strsplit(list, " ", -1);
    GString *json = g_string_new("[");
    int rules = 0;
    for (char **p = paths; *p; p++) {
        if (!**p)
            continue;
        char *escaped = g_regex_escape_string(*p, -1);
        /* JSON string: backslashes doubled. */
        GString *quoted = g_string_new(NULL);
        for (const char *c = escaped; *c; c++) {
            if (*c == '\\' || *c == '"')
                g_string_append_c(quoted, '\\');
            g_string_append_c(quoted, *c);
        }
        g_string_append_printf(json, "%s{\"trigger\":{\"url-filter\":\"%s\"},\"action\":{\"type\":\"block\"}}",
                               rules ? "," : "", quoted->str);
        g_string_free(quoted, TRUE);
        g_free(escaped);
        rules++;
    }
    g_string_append(json, "]");
    g_strfreev(paths);
    if (rules == 0) {
        g_string_free(json, TRUE);
        webkit_user_content_manager_remove_all_filters(S.content);
        if (webkit_web_view_get_uri(S.view))
            webkit_web_view_reload(S.view);
        return;
    }
    if (!S.filters) {
        char *dir = g_build_filename(S.data_dir, "filters", NULL);
        S.filters = webkit_user_content_filter_store_new(dir);
        g_free(dir);
    }
    GBytes *source = g_string_free_to_bytes(json);
    S.filter_saving = 1;
    webkit_user_content_filter_store_save(S.filters, "skip", source, NULL, on_filter_saved, NULL);
    g_bytes_unref(source);
    fprintf(stderr, "panelos-web: %d resources skipped\n", rules);
}

/* ---------- commands from the app ---------- */

static void handle_line(char *l)
{
    char arg[2048];
    int a = 0, b = 0, d = 0, e = 0;
    char c = 0;
    if (sscanf(l, "LOAD %2047s", arg) == 1) {
        set_origin(arg);
        set_kiosk_script();
        if (S.filter_saving) {
            g_free(S.waiting_uri);
            S.waiting_uri = g_strdup(arg);
        } else {
            webkit_web_view_load_uri(S.view, arg);
        }
    } else if (sscanf(l, "KIOSK %d %d", &a, &b) == 2) {
        S.hide_header = a != 0;
        S.hide_sidebar = b != 0;
    } else if (sscanf(l, "TOKEN %2047s %d", arg, &a) == 2) {
        answer_auth(arg, a);
    } else if (sscanf(l, "TOUCH %c %d %d", &c, &a, &b) == 3) {
        touch(c, a, b);
    } else if (sscanf(l, "ATTOP %d %d", &a, &b) == 2) {
        check_top(a, b);
    } else if (sscanf(l, "VISIBLE %d", &a) == 1) {
        set_visible(a);
    } else if (strcmp(l, "RELOAD") == 0) {
        set_kiosk_script();
        webkit_web_view_reload(S.view);
    } else if (sscanf(l, "ZOOM %d", &a) == 1) {
        if (a >= 25 && a <= 300)
            webkit_web_view_set_zoom_level(S.view, a / 100.0);
    } else if (strncmp(l, "SKIP", 4) == 0 && (l[4] == ' ' || l[4] == 0)) {
        set_skip(l[4] ? l + 5 : "");
    } else if (sscanf(l, "BUFS %d %d %d %d", &a, &b, &d, &e) == 4) {
        import_bufs(a, b, d, e);
    } else if (sscanf(l, "DIRECT %d", &a) == 1) {
        if (S.direct && !a)
            last_frame_to_shm();
        S.direct = a != 0;
    } else if (sscanf(l, "FREE %d", &a) == 1) {
        if (a >= 0 && a < S.nbufs) {
            S.bufs[a].busy = 0;
            if (S.waiting) {
                struct wpe_fdo_egl_exported_image *image = S.waiting;
                S.waiting = NULL;
                if (S.direct)
                    draw_direct(a, image);
                done_with(image);
            }
        }
    } else if (strcmp(l, "SCROLLFIND") == 0) {
        run_script(SCROLL_FIND_JS);
    } else if (sscanf(l, "SCROLLTEST %d %d", &a, &b) == 2) {
        char *js = g_strdup_printf(SCROLL_TEST_JS, a, b);
        run_script(js);
        g_free(js);
    }
}

static gboolean on_socket(GIOChannel *ch, GIOCondition cond, gpointer data)
{
    (void)ch; (void)data;
    if (cond & (G_IO_HUP | G_IO_ERR))
        exit(0);
    char buf[4096];
    char ctl[CMSG_SPACE(8 * sizeof(int))];
    struct iovec iov = { buf, sizeof buf };
    struct msghdr msg = { .msg_iov = &iov, .msg_iovlen = 1, .msg_control = ctl, .msg_controllen = sizeof ctl };
    ssize_t n = recvmsg(S.sock, &msg, MSG_CMSG_CLOEXEC);
    for (struct cmsghdr *c = CMSG_FIRSTHDR(&msg); n > 0 && c; c = CMSG_NXTHDR(&msg, c)) {
        if (c->cmsg_level == SOL_SOCKET && c->cmsg_type == SCM_RIGHTS) {
            int count = (c->cmsg_len - CMSG_LEN(0)) / sizeof(int);
            for (int k = 0; k < count; k++) {
                int fd;
                memcpy(&fd, CMSG_DATA(c) + k * sizeof(int), sizeof(int));
                if (S.nfds < 8)
                    S.fds[S.nfds++] = fd;
                else
                    close(fd);
            }
        }
    }
    if (n <= 0) {
        if (n < 0 && errno == EAGAIN)
            return TRUE;
        exit(0);
    }
    for (ssize_t i = 0; i < n; i++) {
        if (buf[i] == '\n') {
            handle_line(S.line->str);
            g_string_truncate(S.line, 0);
        } else if (S.line->len < 4096) {
            g_string_append_c(S.line, buf[i]);
        }
    }
    return TRUE;
}

/* ---------- is the page ready to be shown? ---------- */

/* Home Assistant's dashboard (hui-root, through the shadow roots): "<elements>
 * <pending> <cards>", where pending counts icons without their drawing yet and
 * images not loaded yet, cards the drawn cards (ha-card); "-1 0 0" while it
 * is missing or a card still shows a spinner; "-2 0 0" for a page that is not
 * Home Assistant. */
static const char READY_JS[] =
    "(() => {"
    "  if (!document.querySelector('home-assistant')) return document.readyState === 'complete' ? '-2 0 0' : '-1 0 0';"
    "  const find = (r, tag) => {"
    "    const d = r.querySelector(tag);"
    "    if (d) return d;"
    "    for (const el of r.querySelectorAll('*')) if (el.shadowRoot) { const f = find(el.shadowRoot, tag); if (f) return f; }"
    "    return null;"
    "  };"
    "  const root = find(document, 'hui-root');"
    "  if (!root || !root.shadowRoot) return '-1 0 0';"
    "  let n = 0, busy = 0, pending = 0, cards = 0;"
    "  const walk = (r) => {"
    "    for (const el of r.querySelectorAll('*')) {"
    "      n++;"
    "      const t = el.tagName;"
    "      if (t === 'HA-SPINNER' || t === 'HA-CIRCULAR-PROGRESS') busy++;"
    "      else if (t === 'HA-CARD') cards++;"
    "      else if (t === 'HA-SVG-ICON') { const d = el.shadowRoot && el.shadowRoot.querySelector('path'); if (!d || !d.getAttribute('d')) pending++; }"
    "      else if (t === 'IMG' && !el.complete) pending++;"
    "      if (el.shadowRoot) walk(el.shadowRoot);"
    "    }"
    "  };"
    "  walk(root.shadowRoot);"
    "  return busy ? '-1 0 0' : n + ' ' + pending + ' ' + cards;"
    "})()";

static void ready(const char *why)
{
    if (!S.loading)
        return;
    S.loading = 0;
    send_line("READY %s %ums", why, now_ms() - S.load_started);
}

static void on_ready_js(GObject *object, GAsyncResult *result, gpointer data)
{
    (void)data;
    S.ready_check_running = 0;
    JSCValue *value = webkit_web_view_evaluate_javascript_finish(WEBKIT_WEB_VIEW(object), result, NULL);
    long count = -1, pending = 0, cards = 0;
    if (value) {
        char *text = jsc_value_to_string(value);
        if (text && sscanf(text, "%ld %ld %ld", &count, &pending, &cards) < 1)
            count = -1;
        g_free(text);
        g_object_unref(value);
    }
    if (!S.loading)
        return;
    uint32_t now = now_ms();
    int quiet = now - S.last_frame >= 700;
    if (count == -2) {
        if (quiet)
            ready("page");
        return;
    }
    static long logged[3] = { -9, -9, -9 };
    if (count != logged[0] || pending != logged[1] || cards != logged[2]) {
        fprintf(stderr, "panelos-web: page check +%ums: %ld elements, %ld pending, %ld cards\n",
                now - S.load_started, count, pending, cards);
        logged[0] = count, logged[1] = pending, logged[2] = cards;
    }
    S.stable = count >= 0 && count == S.prev_count ? S.stable + 1 : 0;
    S.prev_count = count;
    if (count < 0)
        return;
    if (!S.root_seen)
        S.root_seen = now;
    /* Settled: cards drawn (a view without cards: 10 s after the dashboard
     * appeared; the frontend itself takes many seconds to start), the same
     * elements three times in a row, icons drawn and images in (an icon that
     * never comes is not waited for longer than 20 s), and no new frames for
     * a while (a page that keeps animating counts as settled after 4 s). */
    uint32_t since = now - S.root_seen;
    if (S.stable >= 3 && (cards > 0 || since > 10000) && (pending == 0 || since > 20000) && (quiet || since > 4000))
        ready(pending ? "icons missing" : cards ? "settled" : "no cards");
}

static gboolean check_ready(gpointer data)
{
    (void)data;
    if (!S.loading)
        return G_SOURCE_CONTINUE;
    if (now_ms() - S.load_started > 45000) {
        ready("timeout");
        return G_SOURCE_CONTINUE;
    }
    if (S.load_finished && !S.ready_check_running) {
        S.ready_check_running = 1;
        webkit_web_view_evaluate_javascript(S.view, READY_JS, -1, NULL, NULL, NULL, on_ready_js, NULL);
    }
    return G_SOURCE_CONTINUE;
}

static void on_load_changed(WebKitWebView *v, WebKitLoadEvent ev, gpointer data)
{
    (void)data;
    if (ev == WEBKIT_LOAD_STARTED) {
        S.loading = 1;
        S.load_started = now_ms();
        S.load_finished = 0;
        S.prev_count = -1;
        S.stable = 0;
        S.root_seen = 0;
        send_line("LOADING");
    } else if (ev == WEBKIT_LOAD_FINISHED) {
        S.load_finished = now_ms();
        send_line("LOADED %d", (int)(webkit_web_view_get_estimated_load_progress(v) * 100));
    }
}

static gboolean on_load_failed(WebKitWebView *v, WebKitLoadEvent ev, char *uri, GError *err, gpointer data)
{
    (void)v; (void)ev; (void)uri; (void)data;
    send_line("FAILED %s", err ? err->message : "unknown");
    return FALSE;
}

/* The view stays on Home Assistant's origin: no new windows, and a document
 * of another origin never replaces the page (its response is refused before
 * it is shown, also after a redirect). Navigation decisions come for the
 * frames inside the page too, without saying which frame, so the main
 * document is checked at its response; frames (an iframe card) and what the
 * page loads stay free. */
static gboolean on_decide_policy(WebKitWebView *v, WebKitPolicyDecision *decision, WebKitPolicyDecisionType type,
                                 gpointer data)
{
    (void)v; (void)data;
    if (type == WEBKIT_POLICY_DECISION_TYPE_NEW_WINDOW_ACTION) {
        webkit_policy_decision_ignore(decision);
        return TRUE;
    }
    if (type != WEBKIT_POLICY_DECISION_TYPE_RESPONSE)
        return FALSE;
    WebKitResponsePolicyDecision *response = WEBKIT_RESPONSE_POLICY_DECISION(decision);
    if (!webkit_response_policy_decision_is_main_frame_main_resource(response))
        return FALSE;
    const char *uri = webkit_uri_response_get_uri(webkit_response_policy_decision_get_response(response));
    if (own_origin(uri))
        return FALSE;
    char *o = origin_of(uri);
    fprintf(stderr, "panelos-web: page of another site not opened (%s)\n", o ? o : "no origin");
    g_free(o);
    webkit_policy_decision_ignore(decision);
    ready("other site");  /* the page on the screen stays: no cover over it */
    return TRUE;
}

static gboolean on_crash(WebKitWebView *v, WebKitWebProcessTerminationReason reason, gpointer data)
{
    (void)v; (void)data;
    send_line("FAILED web process ended (%d)", reason);
    exit(1);  /* the app starts the helper again */
}

int main(int argc, char **argv)
{
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--fd") && i + 1 < argc)
            S.sock = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--size") && i + 1 < argc)
            sscanf(argv[++i], "%dx%d", &S.width, &S.height);
        else if (!strcmp(argv[i], "--data") && i + 1 < argc)
            S.data_dir = argv[++i];
    }
    if (S.sock < 0 || S.width < 64 || S.height < 64 || S.width > 4096 || S.height > 4096) {
        fprintf(stderr, "usage: panelos-web --fd N [--size WxH] [--data DIR]\n");
        return 2;
    }
    if (!wpe_loader_init("libWPEBackend-fdo-1.0.so.1") || init_gl() < 0 || init_shm() < 0) {
        send_line("FAILED setup");
        return 1;
    }

    if (init_blit() < 0)
        fprintf(stderr, "panelos-web: no GPU copy into display buffers, frames go through shared memory\n");

    static const struct wpe_view_backend_exportable_fdo_egl_client client = {
        .export_fdo_egl_image = on_egl_image,
        .export_shm_buffer = on_shm_buffer,
    };
    S.exportable = wpe_view_backend_exportable_fdo_egl_create(&client, NULL, S.width, S.height);
    S.backend = wpe_view_backend_exportable_fdo_get_view_backend(S.exportable);
    WebKitWebViewBackend *vb = webkit_web_view_backend_new(S.backend, NULL, NULL);

    /* Development aid: with a file "console-log" in the data directory (root
     * on the panel only), the page's console messages go to the log. */
    char *console_file = g_build_filename(S.data_dir, "console-log", NULL);
    gboolean console_log = g_file_test(console_file, G_FILE_TEST_EXISTS);
    g_free(console_file);

    char *cache = g_build_filename(S.data_dir, "cache", NULL);
    WebKitNetworkSession *session = webkit_network_session_new(S.data_dir, cache);
    S.content = webkit_user_content_manager_new();
    webkit_user_content_manager_register_script_message_handler(S.content, "getExternalAuth", NULL);
    webkit_user_content_manager_register_script_message_handler(S.content, "revokeExternalAuth", NULL);
    webkit_user_content_manager_register_script_message_handler(S.content, "externalBus", NULL);
    g_signal_connect(S.content, "script-message-received::getExternalAuth", G_CALLBACK(on_get_auth), NULL);
    g_signal_connect(S.content, "script-message-received::revokeExternalAuth", G_CALLBACK(on_revoke_auth), NULL);
    g_signal_connect(S.content, "script-message-received::externalBus", G_CALLBACK(on_external_bus), NULL);
    S.auth_callbacks = g_ptr_array_new_with_free_func(g_free);

    WebKitSettings *settings = webkit_settings_new();
    webkit_settings_set_enable_developer_extras(settings, FALSE);
    webkit_settings_set_enable_write_console_messages_to_stdout(settings, console_log);
    webkit_settings_set_enable_page_cache(settings, FALSE);
    S.view = g_object_new(WEBKIT_TYPE_WEB_VIEW, "backend", vb, "network-session", session, "user-content-manager",
                          S.content, "settings", settings, NULL);
    g_signal_connect(S.view, "load-changed", G_CALLBACK(on_load_changed), NULL);
    g_timeout_add(300, check_ready, NULL);
    g_signal_connect(S.view, "load-failed", G_CALLBACK(on_load_failed), NULL);
    g_signal_connect(S.view, "decide-policy", G_CALLBACK(on_decide_policy), NULL);
    g_signal_connect(S.view, "web-process-terminated", G_CALLBACK(on_crash), NULL);
    set_visible(1);

    S.line = g_string_new(NULL);
    fcntl(S.sock, F_SETFL, fcntl(S.sock, F_GETFL) | O_NONBLOCK);
    GIOChannel *ch = g_io_channel_unix_new(S.sock);
    g_io_add_watch(ch, G_IO_IN | G_IO_HUP | G_IO_ERR, on_socket, NULL);

    g_main_loop_run(g_main_loop_new(NULL, FALSE));
    return 0;
}
