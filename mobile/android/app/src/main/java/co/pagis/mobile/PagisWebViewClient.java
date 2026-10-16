package co.pagis.mobile;

import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebView;
import android.webkit.WebResourceResponse;
import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.util.concurrent.TimeUnit;
import okhttp3.OkHttpClient;
import okhttp3.Request;
import okhttp3.Response;
import com.getcapacitor.Bridge;
import com.getcapacitor.BridgeWebViewClient;
import java.util.function.Consumer;

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
 *
 * The client also tells the activity of each failed main-frame load,
 * after Capacitor.
 */
class PagisWebViewClient extends BridgeWebViewClient {

    private final ServerOrigin shown;
    private final Runnable onPageLoaded;
    private final Consumer<String> onLoadFailed;
    private final Consumer<ServerOrigin> moved;
    private final OkHttpClient probe = new OkHttpClient.Builder()
        .followRedirects(false).followSslRedirects(false).callTimeout(5, TimeUnit.SECONDS).build();

    /**
     * @param shown The origin that the bridge shows: the server, or the
     *     app's own origin on the Connect screen.
     * @param onPageLoaded Runs after each main-frame page load.
     * @param onLoadFailed Takes the text of the error of each failed
     *     main-frame load.
     */
    PagisWebViewClient(Bridge bridge, ServerOrigin shown, Runnable onPageLoaded, Consumer<String> onLoadFailed, Consumer<ServerOrigin> moved) {
        super(bridge);
        this.shown = shown;
        this.onPageLoaded = onPageLoaded;
        this.onLoadFailed = onLoadFailed;
        this.moved = moved;
    }

    /** The first load and each reload check for a move before the bridge opens the page. */
    @Override
    public WebResourceResponse shouldInterceptRequest(WebView view, WebResourceRequest request) {
        if (moved != null && request.isForMainFrame() && shown.matches(request.getUrl().toString())) {
            try (Response response = probe.newCall(new Request.Builder()
                .url(shown.serverUrl() + "/api/v1/health").build()).execute()) {
                ServerOrigin target = shown.permanentRedirect(response.code(), response.header("Location"));
                if (target != null) {
                    view.post(() -> moved.accept(target));
                    return new WebResourceResponse("text/plain", "UTF-8", new ByteArrayInputStream(new byte[0]));
                }
            } catch (IOException ex) {
                // The web view gives the Person the result of its own load.
            }
        }
        return super.shouldInterceptRequest(view, request);
    }

    @Override
    public void onReceivedError(WebView view, WebResourceRequest request, WebResourceError error) {
        super.onReceivedError(view, request, error);
        if (request.isForMainFrame()) {
            onLoadFailed.accept(error.getDescription().toString());
        }
    }

    @Override
    public void onPageFinished(WebView view, String url) {
        super.onPageFinished(view, url);
        onPageLoaded.run();
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
