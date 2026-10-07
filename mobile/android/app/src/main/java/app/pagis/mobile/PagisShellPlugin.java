package app.pagis.mobile;

import com.getcapacitor.JSObject;
import com.getcapacitor.Plugin;
import com.getcapacitor.PluginCall;
import com.getcapacitor.PluginMethod;
import com.getcapacitor.annotation.CapacitorPlugin;

/**
 * {@code PagisShell}, the plugin of the app target that the Connect screen
 * calls ({@code mobile/src/shell.ts}).
 */
@CapacitorPlugin(name = "PagisShell")
public class PagisShellPlugin extends Plugin {

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
}
