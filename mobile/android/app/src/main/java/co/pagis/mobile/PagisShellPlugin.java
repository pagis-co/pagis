package co.pagis.mobile;

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

    @PluginMethod
    public void changeServer(PluginCall call) {
        call.resolve();
        MainActivity activity = (MainActivity) getActivity();
        activity.runOnUiThread(activity::changeServer);
    }

    @PluginMethod
    public void getLockScreenAnswers(PluginCall call) {
        JSObject result = new JSObject();
        result.put("on", new ServerStore(getContext()).lockScreenAnswers());
        call.resolve(result);
    }

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
        call.resolve();
        MainActivity activity = (MainActivity) getActivity();
        activity.runOnUiThread(activity::sessionEnded);
    }
}
