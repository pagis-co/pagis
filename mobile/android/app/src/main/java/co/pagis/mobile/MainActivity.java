package co.pagis.mobile;

import android.app.NotificationManager;
import android.content.Intent;
import android.os.Bundle;
import android.util.Log;
import android.webkit.CookieManager;
import androidx.webkit.WebViewFeature;
import com.getcapacitor.BridgeActivity;
import com.getcapacitor.CapConfig;
import com.getcapacitor.PluginHandle;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * The bridge of the Mobile App. It shows the Product App of the stored
 * server, or the bundled Connect screen when no server is stored.
 *
 * Capacitor reads the server URL once, when it makes the bridge. So to
 * open another server the activity starts again, and with it a new bridge.
 */
public class MainActivity extends BridgeActivity {

    /** The action of the Change server item in the menu of the app icon
     *  ({@code res/xml/shortcuts.xml}). */
    static final String ACTION_CHANGE_SERVER = "co.pagis.mobile.CHANGE_SERVER";

    /** The page that the bridge opens first in place of the origin, such
     *  as a Sign-In Link. Nothing stores it. */
    private static final String EXTRA_FIRST_PAGE = "co.pagis.mobile.FIRST_PAGE";

    private static final String TAG = "Pagis";

    /** The server that this bridge shows, or null on the Connect screen. */
    private ServerOrigin server;
    private SessionCopy sessionCopy;
    /** Runs the native requests of the activity off the main thread. */
    private final ExecutorService background = Executors.newSingleThreadExecutor();

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        registerPlugin(PagisShellPlugin.class);
        registerPlugin(PagisPushPlugin.class);
        sessionCopy = SessionCopy.open(this);
        if (ACTION_CHANGE_SERVER.equals(getIntent().getAction())) {
            new ServerStore(this).forget();
            sessionCopy.delete();
        }
        super.onCreate(savedInstanceState);
    }

    /**
     * Make the bridge. The bridge answers only the main frame of the
     * origin that it shows. Capacitor makes that check with the web
     * message listener of the Android System WebView, and with no such
     * listener every frame reaches its legacy bridge. So the app makes no
     * bridge on a web view that has no web message listener.
     */
    @Override
    protected void load() {
        if (!WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) {
            setContentView(R.layout.webview_update);
            return;
        }
        CapConfig.Builder builder = new CapConfig.Builder(this)
            .setUseLegacyBridge(false)
            .setAppendedUserAgentString("Pagis/" + BuildConfig.VERSION_NAME);
        server = new ServerStore(this).server();
        if (server != null) {
            builder.setServerUrl(server.serverUrl());
            String startPath = server.startPath(getIntent().getStringExtra(EXTRA_FIRST_PAGE));
            // A tap on a Notification that starts the app: the bridge opens
            // its place first.
            if (startPath == null) startPath = tappedPlace(getIntent());
            if (startPath != null) builder.setStartPath(startPath);
            // The copy goes back into the cookie store before the first
            // load, so the bridge opens the server signed in.
            SessionCookies.restore(server, cookieJar(), sessionCopy, System.currentTimeMillis());
        }
        getIntent().removeExtra(EXTRA_FIRST_PAGE);
        getIntent().removeExtra(PushNotifier.EXTRA_NAVIGATE);
        config = builder.create();
        super.load();
        // The origin that the bridge shows: the server, or the app's own
        // origin on the Connect screen. The client takes its place before
        // the web view gives the first navigation to a client, because both
        // run on the main thread.
        ServerOrigin shown = ServerOrigin.parse(bridge.getLocalUrl(), BuildConfig.DEBUG);
        if (shown != null) {
            bridge.setWebViewClient(new PagisWebViewClient(bridge, shown, this::followSession));
        }
        // The microphone goes to the main frame of the server alone.
        bridge.getWebView().setWebChromeClient(new PagisChromeClient(bridge, server));
    }

    /**
     * The daemon sends no push when an item leaves the Needs-You Queue, so
     * the app cancels the stale Notifications when it comes to the
     * foreground.
     */
    @Override
    public void onResume() {
        super.onResume();
        ServerOrigin shown = server;
        if (shown == null) return;
        NotificationTray tray = NotificationTray.of(getSystemService(NotificationManager.class));
        StaleNotifications stale = new StaleNotifications(new NeedsYouClient(), sessionCopy, tray);
        background.execute(() -> {
            try {
                stale.clean(shown);
            } catch (NeedsYouException ex) {
                Log.w(TAG, "The app kept its Notifications: " + ex.getMessage());
            }
        });
    }

    @Override
    public void onDestroy() {
        super.onDestroy();
        background.shutdown();
    }

    @Override
    public void onPause() {
        super.onPause();
        CookieManager.getInstance().flush();
        followSession();
    }

    /** Keep the copy of the Session equal to the Session cookie of the server. */
    private void followSession() {
        if (server != null) {
            SessionCookies.follow(server, cookieJar(), sessionCopy, System.currentTimeMillis());
        }
    }

    private static CookieJar cookieJar() {
        return CookieJar.of(CookieManager.getInstance());
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        if (ACTION_CHANGE_SERVER.equals(intent.getAction())) {
            changeServer();
            return;
        }
        // A tap on a Notification while the bridge is open: the Product App
        // moves its router, and the page does not load again.
        String place = tappedPlace(intent);
        PluginHandle shell = bridge == null ? null : bridge.getPlugin("PagisShell");
        if (place != null && shell != null) {
            ((PagisShellPlugin) shell.getInstance()).navigate(place);
        }
    }

    /**
     * The place on the server of a tap on a Notification that this intent
     * carries, or null. An intent that Android gives again when the Person
     * opens the app from the recent apps is no new tap.
     */
    private String tappedPlace(Intent intent) {
        if (server == null || !intent.hasExtra(PushNotifier.EXTRA_NAVIGATE)) return null;
        if ((intent.getFlags() & Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY) != 0) return null;
        return server.place(intent.getStringExtra(PushNotifier.EXTRA_NAVIGATE));
    }

    /** Keep the server, and start the bridge again at it. */
    void open(ServerOrigin server, String firstPage) {
        new ServerStore(this).keep(server);
        restart(new Intent(this, MainActivity.class).putExtra(EXTRA_FIRST_PAGE, firstPage));
    }

    /**
     * Forget the server and the copy of the Session, and start the bridge
     * again on the Connect screen.
     */
    void changeServer() {
        new ServerStore(this).forget();
        sessionCopy.delete();
        server = null;
        restart(new Intent(this, MainActivity.class));
    }

    /**
     * The Session ended: the Person signed out, or the daemon refused the
     * Session. The app opens the Connect screen.
     */
    void sessionEnded() {
        changeServer();
    }

    private void restart(Intent intent) {
        setIntent(intent);
        recreate();
    }
}
