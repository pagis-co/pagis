package co.pagis.mobile;

import android.webkit.CookieManager;

/** The cookie store of the web view, as the copy of the Session reads and writes it. */
interface CookieJar {

    /** The cookies of a URL, as a {@code Cookie} header gives them, or null for none. */
    String getCookie(String url);

    /** Set one cookie of a URL, from the text of a {@code Set-Cookie} header. */
    void setCookie(String url, String cookie);

    /** The cookie store of the web views of the app. */
    static CookieJar of(CookieManager manager) {
        return new CookieJar() {
            @Override
            public String getCookie(String url) {
                return manager.getCookie(url);
            }

            @Override
            public void setCookie(String url, String cookie) {
                manager.setCookie(url, cookie);
            }
        };
    }
}
