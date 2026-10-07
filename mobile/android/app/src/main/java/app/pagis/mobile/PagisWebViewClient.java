package app.pagis.mobile;

import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.webkit.WebResourceRequest;
import android.webkit.WebView;
import com.getcapacitor.Bridge;
import com.getcapacitor.BridgeWebViewClient;

/**
 * The web view client of the bridge.
 *
 * Capacitor keeps a main-frame navigation in the web view when its scheme
 * and its host match the server URL, whatever the port
 * ({@code Bridge.launchIntent}). So {@code https://a.example:444} loads in
 * the app for the server {@code https://a.example}, and the app shows no
 * address. This client opens a main-frame navigation to each {@code http}
 * or {@code https} origin other than the origin that the bridge shows in
 * the system browser. It passes every other navigation to Capacitor
 * (ADR-0032).
 */
class PagisWebViewClient extends BridgeWebViewClient {

    private final ServerOrigin shown;

    /**
     * @param shown The origin that the bridge shows: the server, or the
     *     app's own origin on the Connect screen.
     */
    PagisWebViewClient(Bridge bridge, ServerOrigin shown) {
        super(bridge);
        this.shown = shown;
    }

    @Override
    public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
        if (request.isForMainFrame() && shown.opensOutside(request.getUrl().toString())) {
            try {
                view.getContext().startActivity(new Intent(Intent.ACTION_VIEW, request.getUrl()));
            } catch (ActivityNotFoundException ex) {
                // No app opens the address, and the web view stays where it is.
            }
            return true;
        }
        return super.shouldOverrideUrlLoading(view, request);
    }
}
