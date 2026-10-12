package co.pagis.mobile;

import android.content.Intent;
import android.net.Uri;
import androidx.browser.customtabs.CustomTabsIntent;
import com.getcapacitor.JSObject;
import com.getcapacitor.Plugin;
import com.getcapacitor.PluginCall;
import com.getcapacitor.PluginMethod;
import com.getcapacitor.annotation.CapacitorPlugin;

/**
 * {@code PagisShell}, the plugin of the app target. The Connect screen calls
 * it ({@code mobile/src/shell.ts}), and so does the Product App
 * ({@code ui/src/mobileShell.ts}).
 */
@CapacitorPlugin(name = "PagisShell")
public class PagisShellPlugin extends Plugin {
    private PluginCall authentication;
    private boolean browserOpened;

    static CustomTabsIntent signInTab() {
        return new CustomTabsIntent.Builder().setEphemeralBrowsingEnabled(true).build();
    }

    /** Open the sign-in page in a Custom Tab. */
    @PluginMethod
    public void authenticate(PluginCall call) {
        String text = call.getString("url");
        Uri url = text == null ? null : Uri.parse(text);
        ServerOrigin origin = url == null ? null : ServerOrigin.parse(url.getScheme() + "://" + url.getEncodedAuthority(), BuildConfig.DEBUG);
        if (origin == null || !"/sign-in".equals(url.getPath()) || url.getFragment() != null) {
            call.reject("This is not a sign-in page of a trusted server.");
            return;
        }
        getActivity().runOnUiThread(() -> {
            if (authentication != null) authentication.reject("Sign-in was replaced. Connect again.");
            authentication = call;
            browserOpened = false;
            try {
                signInTab().launchUrl(getActivity(), url);
            } catch (android.content.ActivityNotFoundException error) {
                authentication = null;
                call.reject("No system browser opens sign-in. Install a browser, then connect again.");
            }
        });
    }

    @Override
    protected void handleOnNewIntent(Intent intent) {
        Uri url = intent.getData();
        if (url == null || !"pagis-app".equals(url.getScheme()) || !"signed-in".equals(url.getHost())) return;
        if (authentication == null) return;
        PluginCall call = authentication;
        authentication = null;
        JSObject answer = new JSObject();
        answer.put("url", url.toString());
        call.resolve(answer);
    }

    @Override
    protected void handleOnPause() {
        if (authentication != null) browserOpened = true;
    }

    @Override
    protected void handleOnResume() {
        if (authentication != null && browserOpened) {
            PluginCall call = authentication;
            authentication = null;
            browserOpened = false;
            call.reject("Sign-in was closed. Connect again.");
        }
    }

    /** Give the verifier only to the bridge on its server and for its secret. */
    @PluginMethod
    public void takeSignInVerifier(PluginCall call) {
        ServerOrigin server = ServerOrigin.parse(getBridge().getConfig().getServerUrl(), BuildConfig.DEBUG);
        String value = SignInVerifier.take(server == null ? null : server.serverUrl(), call.getString("secret", ""));
        JSObject result = new JSObject();
        if (value != null) result.put("verifier", value);
        call.resolve(result);
    }


    /**
     * Change server of the You screen. The app forgets the server and the
     * copy of the Session, and shows the Connect screen.
     */
    @PluginMethod
    public void changeServer(PluginCall call) {
        SignInVerifier.clear();
        call.resolve();
        MainActivity activity = (MainActivity) getActivity();
        activity.runOnUiThread(activity::changeServer);
    }

    /** The setting Answer on the lock screen of this phone. */
    @PluginMethod
    public void getLockScreenAnswers(PluginCall call) {
        JSObject result = new JSObject();
        result.put("on", new ServerStore(getContext()).lockScreenAnswers());
        call.resolve(result);
    }

    /**
     * Keep the setting Answer on the lock screen. The next Notification of
     * an Approval shows its actions, or none, by the setting (ADR-0032).
     */
    @PluginMethod
    public void setLockScreenAnswers(PluginCall call) {
        Boolean on = call.getBoolean("on");
        if (on == null) { call.reject("The setting needs an on or off value."); return; }
        new ServerStore(getContext()).setLockScreenAnswers(on);
        call.resolve();
    }

    @PluginMethod
    public void buildType(PluginCall call) {
        JSObject result = new JSObject();
        result.put("debug", BuildConfig.DEBUG);
        call.resolve(result);
    }

    /**
     * Keep the origin, and start the bridge again at the server with the
     * first page {@code opens}. The shell checks both again: the page
     * asks, and the native side decides.
     */
    @PluginMethod
    public void open(PluginCall call) {
        ServerOrigin server = ServerOrigin.parse(call.getString("origin"), BuildConfig.DEBUG);
        if (server == null) {
            call.reject("This is not the origin of a Pagis server that the app opens.");
            return;
        }
        String opens = call.getString("opens");
        if (server.startPath(opens) == null) {
            call.reject("The first page is not on the origin of the server.");
            return;
        }
        SignInVerifier.clear();
        String verifier = call.getString("verifier");
        Uri page = Uri.parse(opens);
        if (verifier != null && "/sign-in".equals(page.getPath()) && page.getFragment() != null) {
            SignInVerifier.keep(server.serverUrl(), page.getFragment(), verifier);
        }
        call.resolve();
        MainActivity activity = (MainActivity) getActivity();
        activity.runOnUiThread(() -> activity.open(server, opens));
    }

    /**
     * Send the Product App the event {@code navigate} with the place of a
     * tap on a Notification, and the Product App moves its router. The page
     * does not load again. An event that comes before the Product App
     * listens waits for its first listener.
     */
    void navigate(String place) {
        JSObject data = new JSObject();
        data.put("path", place);
        notifyListeners("navigate", data, true);
    }

    /**
     * The Product App says that the Session ended. The shell deletes the
     * copy of the Session and opens the Connect screen.
     */
    @PluginMethod
    public void sessionEnded(PluginCall call) {
        SignInVerifier.clear();
        call.resolve();
        MainActivity activity = (MainActivity) getActivity();
        activity.runOnUiThread(activity::sessionEnded);
    }
}
