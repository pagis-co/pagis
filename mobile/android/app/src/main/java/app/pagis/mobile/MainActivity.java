package app.pagis.mobile;

import android.content.Intent;
import android.os.Bundle;
import android.webkit.CookieManager;
import androidx.webkit.WebViewFeature;
import com.getcapacitor.BridgeActivity;
import com.getcapacitor.CapConfig;

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
    static final String ACTION_CHANGE_SERVER = "app.pagis.mobile.CHANGE_SERVER";

    /** The page that the bridge opens first in place of the origin, such
     *  as a Sign-In Link. Nothing stores it. */
    private static final String EXTRA_FIRST_PAGE = "app.pagis.mobile.FIRST_PAGE";

    /** The server that this bridge shows, or null on the Connect screen. */
    private ServerOrigin server;
    private SessionCopy sessionCopy;

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
            if (startPath != null) builder.setStartPath(startPath);
            // The copy goes back into the cookie store before the first
            // load, so the bridge opens the server signed in.
            SessionCookies.restore(server, cookieJar(), sessionCopy, System.currentTimeMillis());
        }
        getIntent().removeExtra(EXTRA_FIRST_PAGE);
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
        }
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
