package co.pagis.mobile;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.content.Context;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;

/**
 * {@link ServerStore} keeps the server that the app opens and the setting
 * Answer on the lock screen of this phone (ADR-0032).
 */
@RunWith(RobolectricTestRunner.class)
public class ServerStoreTest {

    private static final ServerOrigin SERVER = ServerOrigin.parse("https://pagis.example.com", false);

    private Context context;

    @Before
    public void findContext() {
        context = RuntimeEnvironment.getApplication();
    }

    @Test
    public void theServerIsKeptUntilTheAppForgetsIt() {
        assertNull(new ServerStore(context).server());

        new ServerStore(context).keep(SERVER);
        assertEquals(SERVER, new ServerStore(context).server());

        new ServerStore(context).forget();
        assertNull(new ServerStore(context).server());
    }

    @Test
    public void lockScreenAnswersAreOffWhenThePhoneStoresNoSetting() {
        assertFalse(new ServerStore(context).lockScreenAnswers());
    }

    @Test
    public void theStoreKeepsLockScreenAnswers() {
        new ServerStore(context).setLockScreenAnswers(true);
        assertTrue(new ServerStore(context).lockScreenAnswers());

        new ServerStore(context).setLockScreenAnswers(false);
        assertFalse(new ServerStore(context).lockScreenAnswers());
    }

    /**
     * Change server forgets the server. Answer on the lock screen is a
     * setting of the phone, so it stays.
     */
    @Test
    public void forgettingTheServerKeepsLockScreenAnswers() {
        new ServerStore(context).keep(SERVER);
        new ServerStore(context).setLockScreenAnswers(true);

        new ServerStore(context).forget();

        assertNull(new ServerStore(context).server());
        assertTrue(new ServerStore(context).lockScreenAnswers());
    }
}
