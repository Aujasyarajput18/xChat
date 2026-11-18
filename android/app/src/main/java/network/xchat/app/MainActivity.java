package network.xchat.app;

import android.Manifest;
import android.app.Activity;
import android.content.pm.PackageManager;
import android.graphics.Color;
import android.os.Build;
import android.os.Bundle;
import android.view.View;
import android.view.Window;
import android.view.WindowManager;
import android.webkit.ConsoleMessage;
import android.webkit.WebChromeClient;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import java.util.ArrayList;
import java.util.List;

public class MainActivity extends Activity {
    private WebView webView;
    private AndroidBridge bridge;
    private static final int PERMISSION_REQ_CODE = 1001;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        // Configure edge-to-edge Telegram dark theme for status and navigation bars
        Window window = getWindow();
        window.clearFlags(WindowManager.LayoutParams.FLAG_TRANSLUCENT_STATUS);
        window.addFlags(WindowManager.LayoutParams.FLAG_DRAWS_SYSTEM_BAR_BACKGROUNDS);
        window.setStatusBarColor(Color.parseColor("#17212b"));
        window.setNavigationBarColor(Color.parseColor("#17212b"));

        webView = new WebView(this);
        webView.setBackgroundColor(Color.parseColor("#0e1621"));
        setContentView(webView);

        configureWebView();
        checkAndRequestPermissions();

        // Load the embedded local offline application
        webView.loadUrl("file:///android_asset/index.html");
    }

    private void configureWebView() {
        WebSettings settings = webView.getSettings();
        settings.setJavaScriptEnabled(true);
        settings.setDomStorageEnabled(true);
        settings.setDatabaseEnabled(true);
        settings.setAllowFileAccess(true);
        settings.setAllowContentAccess(true);
        settings.setAllowFileAccessFromFileURLs(false);
        settings.setAllowUniversalAccessFromFileURLs(false);
        settings.setLoadsImagesAutomatically(true);
        settings.setUseWideViewPort(true);
        settings.setLoadWithOverviewMode(true);
        settings.setSupportZoom(false);

        bridge = new AndroidBridge(this);
        webView.addJavascriptInterface(bridge, "AndroidBridge");

        webView.setWebViewClient(new WebViewClient() {
            @Override
            public boolean shouldOverrideUrlLoading(WebView view, String url) {
                if (url.startsWith("file://") || url.startsWith("http://127.0.0.1") || url.startsWith("http://localhost")) {
                    return false;
                }
                return false;
            }
        });

        webView.setWebChromeClient(new WebChromeClient() {
            @Override
            public boolean onConsoleMessage(ConsoleMessage consoleMessage) {
                return true;
            }

            @Override
            public void onPermissionRequest(final android.webkit.PermissionRequest request) {
                request.grant(request.getResources());
            }

            @Override
            public boolean onJsAlert(WebView view, String url, String message, final android.webkit.JsResult result) {
                new android.app.AlertDialog.Builder(MainActivity.this)
                    .setTitle("xChat")
                    .setMessage(message)
                    .setPositiveButton(android.R.string.ok, new android.content.DialogInterface.OnClickListener() {
                        @Override
                        public void onClick(android.content.DialogInterface dialog, int which) {
                            result.confirm();
                        }
                    })
                    .setOnCancelListener(new android.content.DialogInterface.OnCancelListener() {
                        @Override
                        public void onCancel(android.content.DialogInterface dialog) {
                            result.cancel();
                        }
                    })
                    .show();
                return true;
            }

            @Override
            public boolean onJsConfirm(WebView view, String url, String message, final android.webkit.JsResult result) {
                new android.app.AlertDialog.Builder(MainActivity.this)
                    .setTitle("xChat")
                    .setMessage(message)
                    .setPositiveButton(android.R.string.ok, new android.content.DialogInterface.OnClickListener() {
                        @Override
                        public void onClick(android.content.DialogInterface dialog, int which) {
                            result.confirm();
                        }
                    })
                    .setNegativeButton(android.R.string.cancel, new android.content.DialogInterface.OnClickListener() {
                        @Override
                        public void onClick(android.content.DialogInterface dialog, int which) {
                            result.cancel();
                        }
                    })
                    .setOnCancelListener(new android.content.DialogInterface.OnCancelListener() {
                        @Override
                        public void onCancel(android.content.DialogInterface dialog) {
                            result.cancel();
                        }
                    })
                    .show();
                return true;
            }
        });
    }

    private void checkAndRequestPermissions() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            List<String> neededPermissions = new ArrayList<>();

            if (checkSelfPermission(Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
                neededPermissions.add(Manifest.permission.CAMERA);
            }

            if (checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION) != PackageManager.PERMISSION_GRANTED) {
                neededPermissions.add(Manifest.permission.ACCESS_FINE_LOCATION);
            }

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                if (checkSelfPermission(Manifest.permission.BLUETOOTH_SCAN) != PackageManager.PERMISSION_GRANTED) {
                    neededPermissions.add(Manifest.permission.BLUETOOTH_SCAN);
                }
                if (checkSelfPermission(Manifest.permission.BLUETOOTH_CONNECT) != PackageManager.PERMISSION_GRANTED) {
                    neededPermissions.add(Manifest.permission.BLUETOOTH_CONNECT);
                }
                if (checkSelfPermission(Manifest.permission.BLUETOOTH_ADVERTISE) != PackageManager.PERMISSION_GRANTED) {
                    neededPermissions.add(Manifest.permission.BLUETOOTH_ADVERTISE);
                }
            }

            if (!neededPermissions.isEmpty()) {
                requestPermissions(neededPermissions.toArray(new String[0]), PERMISSION_REQ_CODE);
            }
        }
    }

    public WebView getWebView() {
        return webView;
    }

    @Override
    protected void onDestroy() {
        if (bridge != null) {
            bridge.cleanup();
        }
        super.onDestroy();
    }

    @Override
    public void onBackPressed() {
        // If chat room is active in WebView, return to contact list first
        if (webView != null) {
            webView.evaluateJavascript("if (typeof handleBackPressed === 'function') { handleBackPressed(); } else { false; }", new android.webkit.ValueCallback<String>() {
                @Override
                public void onReceiveValue(String value) {
                    if (!"true".equals(value)) {
                        MainActivity.super.onBackPressed();
                    }
                }
            });
        } else {
            super.onBackPressed();
        }
    }
}
