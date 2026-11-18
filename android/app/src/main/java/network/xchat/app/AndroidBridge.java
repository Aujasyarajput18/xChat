package network.xchat.app;

import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.net.wifi.WifiManager;
import android.os.Build;
import android.os.VibrationEffect;
import android.os.Vibrator;
import android.os.VibratorManager;
import android.webkit.JavascriptInterface;
import android.widget.Toast;
import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.HttpURLConnection;
import java.net.Inet4Address;
import java.net.InetAddress;
import java.net.NetworkInterface;
import java.net.ServerSocket;
import java.net.Socket;
import java.net.URL;
import java.util.Collections;
import java.util.Enumeration;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import org.json.JSONArray;
import org.json.JSONObject;

public class AndroidBridge {
    private final MainActivity activity;
    private final SharedPreferences prefs;
    private ServerSocket serverSocket;
    private int listenPort = 9400;
    private volatile boolean isServerRunning = false;

    // UDP Peer Discovery
    private static final int DISCOVERY_PORT = 9444;
    private DatagramSocket udpSocket;
    private volatile boolean isDiscoveryRunning = false;
    private WifiManager.MulticastLock multicastLock;
    private final Map<String, DiscoveredPeerInfo> discoveredPeers = new ConcurrentHashMap<>();

    private String myUsername = "";
    private String myDisplayName = "";
    private String myMeshAddress = "";
    private String myFingerprint = "";

    public static class DiscoveredPeerInfo {
        public String username;
        public String displayName;
        public String address;
        public String fingerprint;
        public String ip;
        public int port;
        public long lastSeen;

        public JSONObject toJson() {
            try {
                JSONObject obj = new JSONObject();
                obj.put("username", username);
                obj.put("display_name", displayName);
                obj.put("address", address);
                obj.put("fingerprint", fingerprint);
                obj.put("ip", ip);
                obj.put("port", port);
                obj.put("last_seen", lastSeen);
                return obj;
            } catch (Exception e) {
                return new JSONObject();
            }
        }
    }

    public AndroidBridge(MainActivity activity) {
        this.activity = activity;
        this.prefs = activity.getSharedPreferences("xchat_prefs", Context.MODE_PRIVATE);
        this.myUsername = prefs.getString("username", "user_" + (new java.security.SecureRandom().nextInt(90000) + 10000));
        this.myDisplayName = prefs.getString("display_name", "Android Node");
        this.myMeshAddress = prefs.getString("my_address", "xchat_android");
        this.myFingerprint = prefs.getString("fingerprint", "");

        startReceiverServer();
        startDiscoveryEngine();
    }

    public void cleanup() {
        isServerRunning = false;
        isDiscoveryRunning = false;
        if (serverSocket != null) {
            try {
                serverSocket.close();
            } catch (Exception ignored) {}
        }
        if (udpSocket != null) {
            try {
                udpSocket.close();
            } catch (Exception ignored) {}
        }
        if (multicastLock != null && multicastLock.isHeld()) {
            try {
                multicastLock.release();
            } catch (Exception ignored) {}
        }
    }

    private void startDiscoveryEngine() {
        try {
            WifiManager wm = (WifiManager) activity.getApplicationContext().getSystemService(Context.WIFI_SERVICE);
            if (wm != null) {
                multicastLock = wm.createMulticastLock("xchat_discovery_lock");
                multicastLock.setReferenceCounted(true);
                multicastLock.acquire();
            }
        } catch (Exception ignored) {}

        isDiscoveryRunning = true;

        // UDP Receiver Thread
        new Thread(new Runnable() {
            @Override
            public void run() {
                try {
                    try {
                        udpSocket = new DatagramSocket(DISCOVERY_PORT);
                    } catch (Exception portTaken) {
                        udpSocket = new DatagramSocket(0);
                    }
                    udpSocket.setBroadcast(true);

                    byte[] buffer = new byte[2048];
                    while (isDiscoveryRunning && !udpSocket.isClosed()) {
                        DatagramPacket packet = new DatagramPacket(buffer, buffer.length);
                        udpSocket.receive(packet);
                        String raw = new String(packet.getData(), 0, packet.getLength(), "UTF-8");
                        handleDiscoveryPacket(raw, packet.getAddress().getHostAddress());
                    }
                } catch (Exception ignored) {}
            }
        }).start();

        // Periodic UDP Beacon Broadcast Thread
        new Thread(new Runnable() {
            @Override
            public void run() {
                while (isDiscoveryRunning) {
                    try {
                        broadcastBeacon();
                        Thread.sleep(4000);
                    } catch (Exception ignored) {}
                }
            }
        }).start();
    }

    private void handleDiscoveryPacket(String jsonStr, String senderIp) {
        try {
            JSONObject obj = new JSONObject(jsonStr);
            String u = obj.optString("username", "");
            String addr = obj.optString("address", "");
            if (addr.equals(myMeshAddress)) {
                return; // Ignore self broadcast
            }
            DiscoveredPeerInfo peer = new DiscoveredPeerInfo();
            peer.username = u;
            peer.displayName = obj.optString("display_name", u);
            peer.address = addr;
            peer.fingerprint = obj.optString("fingerprint", "");
            peer.port = obj.optInt("port", 9400);
            peer.ip = senderIp;
            peer.lastSeen = System.currentTimeMillis();

            discoveredPeers.put(addr, peer);

            // Notify WebView
            activity.runOnUiThread(new Runnable() {
                @Override
                public void run() {
                    try {
                        String js = "if (window.onNearbyPeersUpdated) { window.onNearbyPeersUpdated(); }";
                        activity.getWebView().evaluateJavascript(js, null);
                    } catch (Exception ignored) {}
                }
            });
        } catch (Exception ignored) {}
    }

    private void broadcastBeacon() {
        if (udpSocket == null || udpSocket.isClosed()) return;
        try {
            JSONObject obj = new JSONObject();
            obj.put("username", myUsername.startsWith("@") ? myUsername : "@" + myUsername);
            obj.put("display_name", myDisplayName);
            obj.put("address", myMeshAddress);
            obj.put("fingerprint", myFingerprint);
            obj.put("port", listenPort);
            obj.put("timestamp", System.currentTimeMillis() / 1000);

            byte[] bytes = obj.toString().getBytes("UTF-8");
            DatagramPacket packet = new DatagramPacket(
                bytes,
                bytes.length,
                InetAddress.getByName("255.255.255.255"),
                DISCOVERY_PORT
            );
            udpSocket.send(packet);
        } catch (Exception ignored) {}
    }

    @JavascriptInterface
    public void updateDiscoveryProfile(String username, String displayName, String address, String fingerprint) {
        this.myUsername = username != null ? username : "";
        this.myDisplayName = displayName != null ? displayName : "";
        this.myMeshAddress = address != null ? address : "";
        this.myFingerprint = fingerprint != null ? fingerprint : "";

        prefs.edit()
            .putString("username", this.myUsername)
            .putString("display_name", this.myDisplayName)
            .putString("my_address", this.myMeshAddress)
            .putString("fingerprint", this.myFingerprint)
            .apply();

        // Trigger immediate beacon broadcast
        new Thread(new Runnable() {
            @Override
            public void run() {
                broadcastBeacon();
            }
        }).start();
    }

    @JavascriptInterface
    public String getDiscoveredPeers() {
        JSONArray arr = new JSONArray();
        long now = System.currentTimeMillis();
        for (DiscoveredPeerInfo peer : discoveredPeers.values()) {
            // Expire peers after 60s
            if (now - peer.lastSeen < 60000) {
                arr.put(peer.toJson());
            }
        }
        return arr.toString();
    }

    @JavascriptInterface
    public String resolveUsername(String username) {
        if (username == null) return "{}";
        String clean = username.trim();
        if (!clean.startsWith("@")) clean = "@" + clean;
        long now = System.currentTimeMillis();
        for (DiscoveredPeerInfo peer : discoveredPeers.values()) {
            if (now - peer.lastSeen < 60000 && peer.username.equalsIgnoreCase(clean)) {
                return peer.toJson().toString();
            }
        }
        return "{}";
    }

    private void startReceiverServer() {
        isServerRunning = true;
        new Thread(new Runnable() {
            @Override
            public void run() {
                try {
                    try {
                        serverSocket = new ServerSocket(listenPort);
                    } catch (Exception portTaken) {
                        serverSocket = new ServerSocket(0);
                        listenPort = serverSocket.getLocalPort();
                    }
                    while (isServerRunning && !serverSocket.isClosed()) {
                        final Socket client = serverSocket.accept();
                        new Thread(new Runnable() {
                            @Override
                            public void run() {
                                handleClient(client);
                            }
                        }).start();
                    }
                } catch (Exception ignored) {}
            }
        }).start();
    }

    private void handleClient(Socket socket) {
        try {
            BufferedReader reader = new BufferedReader(new InputStreamReader(socket.getInputStream(), "UTF-8"));
            String line = reader.readLine();
            if (line == null) {
                socket.close();
                return;
            }

            String[] parts = line.split(" ");
            String method = parts.length > 0 ? parts[0] : "GET";
            String path = parts.length > 1 ? parts[1] : "/";

            int contentLength = 0;
            while ((line = reader.readLine()) != null && !line.isEmpty()) {
                if (line.toLowerCase().startsWith("content-length:")) {
                    try {
                        contentLength = Integer.parseInt(line.substring(15).trim());
                    } catch (NumberFormatException ignored) {
                        contentLength = 0;
                    }
                }
            }

            // Cap maximum payload at 1 MB to eliminate memory exhaustion DoS
            final int MAX_BODY_SIZE = 1024 * 1024;
            if (contentLength < 0 || contentLength > MAX_BODY_SIZE) {
                socket.close();
                return;
            }

            StringBuilder bodyBuilder = new StringBuilder();
            if (contentLength > 0) {
                char[] chunk = new char[Math.min(contentLength, 4096)];
                int totalRead = 0;
                while (totalRead < contentLength) {
                    int toRead = Math.min(chunk.length, contentLength - totalRead);
                    int r = reader.read(chunk, 0, toRead);
                    if (r == -1) break;
                    bodyBuilder.append(chunk, 0, r);
                    totalRead += r;
                }
            }
            final String body = bodyBuilder.toString();

            OutputStream os = socket.getOutputStream();
            if ("OPTIONS".equalsIgnoreCase(method)) {
                String resp = "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nContent-Length: 0\r\n\r\n";
                os.write(resp.getBytes("UTF-8"));
            } else if (path.startsWith("/api/identity")) {
                String myAddr = prefs.getString("my_address", myMeshAddress);
                String respBody = "{\"address\":\"" + myAddr + "\",\"username\":\"" + myUsername + "\",\"display_name\":\"" + myDisplayName + "\",\"fingerprint\":\"" + myFingerprint + "\",\"ip\":\"" + getLocalIpAddress() + "\",\"port\":" + listenPort + ",\"status\":\"online\"}";
                String resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: " + respBody.getBytes("UTF-8").length + "\r\n\r\n" + respBody;
                os.write(resp.getBytes("UTF-8"));
            } else if (path.startsWith("/api/receive")) {
                if (!body.isEmpty()) {
                    activity.runOnUiThread(new Runnable() {
                        @Override
                        public void run() {
                            vibrate(25);
                            try {
                                String js = "if (window.onIncomingNetworkMessage) { window.onIncomingNetworkMessage(" + JSONObject.quote(body) + "); }";
                                activity.getWebView().evaluateJavascript(js, null);
                            } catch (Exception ignored) {}
                        }
                    });
                }
                String respBody = "{\"status\":\"delivered\",\"timestamp\":" + System.currentTimeMillis() + "}";
                String resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: " + respBody.getBytes("UTF-8").length + "\r\n\r\n" + respBody;
                os.write(resp.getBytes("UTF-8"));
            } else {
                String respBody = "{\"status\":\"ok\"}";
                String resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: " + respBody.getBytes("UTF-8").length + "\r\n\r\n" + respBody;
                os.write(resp.getBytes("UTF-8"));
            }
            os.flush();
            socket.close();
        } catch (Exception ignored) {
            try { socket.close(); } catch (Exception e) {}
        }
    }

    @JavascriptInterface
    public String getLocalIpAddress() {
        try {
            Enumeration<NetworkInterface> interfaces = NetworkInterface.getNetworkInterfaces();
            while (interfaces.hasMoreElements()) {
                NetworkInterface iface = interfaces.nextElement();
                if (iface.isLoopback() || !iface.isUp()) continue;
                Enumeration<InetAddress> addrs = iface.getInetAddresses();
                while (addrs.hasMoreElements()) {
                    InetAddress addr = addrs.nextElement();
                    if (!addr.isLoopbackAddress() && addr instanceof Inet4Address) {
                        return addr.getHostAddress();
                    }
                }
            }
        } catch (Exception ignored) {}
        return "127.0.0.1";
    }

    @JavascriptInterface
    public String getReachableAddress() {
        return getLocalIpAddress() + ":" + listenPort;
    }

    @JavascriptInterface
    public int getListenPort() {
        return listenPort;
    }

    @JavascriptInterface
    public void sendHttpEnvelope(final String targetHostPort, final String jsonPayload, final String callbackId) {
        new Thread(new Runnable() {
            @Override
            public void run() {
                String cleanTarget = targetHostPort.trim();
                String urlStr = cleanTarget.startsWith("http://") || cleanTarget.startsWith("https://")
                    ? cleanTarget
                    : "http://" + cleanTarget + "/api/receive";
                try {
                    URL url = new URL(urlStr);
                    HttpURLConnection conn = (HttpURLConnection) url.openConnection();
                    conn.setRequestMethod("POST");
                    conn.setRequestProperty("Content-Type", "application/json");
                    conn.setConnectTimeout(4000);
                    conn.setReadTimeout(4000);
                    conn.setDoOutput(true);
                    byte[] bytes = jsonPayload.getBytes("UTF-8");
                    OutputStream os = conn.getOutputStream();
                    os.write(bytes);
                    os.flush();
                    os.close();
                    int code = conn.getResponseCode();
                    final boolean success = (code >= 200 && code < 300);
                    activity.runOnUiThread(new Runnable() {
                        @Override
                        public void run() {
                            String js = "if(window.onSendCallback) window.onSendCallback(" + JSONObject.quote(callbackId) + ", " + success + ");";
                            activity.getWebView().evaluateJavascript(js, null);
                        }
                    });
                } catch (Exception e) {
                    activity.runOnUiThread(new Runnable() {
                        @Override
                        public void run() {
                            String js = "if(window.onSendCallback) window.onSendCallback(" + JSONObject.quote(callbackId) + ", false);";
                            activity.getWebView().evaluateJavascript(js, null);
                        }
                    });
                }
            }
        }).start();
    }

    @JavascriptInterface
    public void pingPeer(final String targetHostPort, final String callbackId) {
        new Thread(new Runnable() {
            @Override
            public void run() {
                long start = System.currentTimeMillis();
                String cleanTarget = targetHostPort.trim();
                String urlStr = cleanTarget.startsWith("http://") || cleanTarget.startsWith("https://")
                    ? cleanTarget
                    : "http://" + cleanTarget + "/api/identity";
                try {
                    URL url = new URL(urlStr);
                    HttpURLConnection conn = (HttpURLConnection) url.openConnection();
                    conn.setRequestMethod("GET");
                    conn.setConnectTimeout(3000);
                    conn.setReadTimeout(3000);
                    int code = conn.getResponseCode();
                    long rtt = System.currentTimeMillis() - start;
                    final boolean ok = (code >= 200 && code < 400);
                    final long latency = ok ? rtt : -1;
                    activity.runOnUiThread(new Runnable() {
                        @Override
                        public void run() {
                            String js = "if(window.onPingCallback) window.onPingCallback(" + JSONObject.quote(callbackId) + ", " + latency + ");";
                            activity.getWebView().evaluateJavascript(js, null);
                        }
                    });
                } catch (Exception e) {
                    activity.runOnUiThread(new Runnable() {
                        @Override
                        public void run() {
                            String js = "if(window.onPingCallback) window.onPingCallback(" + JSONObject.quote(callbackId) + ", -1);";
                            activity.getWebView().evaluateJavascript(js, null);
                        }
                    });
                }
            }
        }).start();
    }

    @JavascriptInterface
    public void toast(final String message) {
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                Toast.makeText(activity, message, Toast.LENGTH_SHORT).show();
            }
        });
    }

    @JavascriptInterface
    public void copyToClipboard(final String text) {
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                ClipboardManager clipboard = (ClipboardManager) activity.getSystemService(Context.CLIPBOARD_SERVICE);
                if (clipboard != null) {
                    ClipData clip = ClipData.newPlainText("xChat Address", text);
                    clipboard.setPrimaryClip(clip);
                    Toast.makeText(activity, "Copied to clipboard!", Toast.LENGTH_SHORT).show();
                }
            }
        });
    }

    @JavascriptInterface
    public void shareAddress(final String address) {
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                Intent sendIntent = new Intent();
                sendIntent.setAction(Intent.ACTION_SEND);
                sendIntent.putExtra(Intent.EXTRA_TEXT, "Connect with me on xChat private mesh:\n" + address);
                sendIntent.setType("text/plain");
                Intent shareIntent = Intent.createChooser(sendIntent, "Share xChat Identity");
                activity.startActivity(shareIntent);
            }
        });
    }

    @JavascriptInterface
    public void vibrate(long ms) {
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                VibratorManager vm = (VibratorManager) activity.getSystemService(Context.VIBRATOR_MANAGER_SERVICE);
                if (vm != null) {
                    Vibrator v = vm.getDefaultVibrator();
                    v.vibrate(VibrationEffect.createOneShot(ms, VibrationEffect.DEFAULT_AMPLITUDE));
                }
            } else {
                Vibrator v = (Vibrator) activity.getSystemService(Context.VIBRATOR_SERVICE);
                if (v != null) {
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                        v.vibrate(VibrationEffect.createOneShot(ms, VibrationEffect.DEFAULT_AMPLITUDE));
                    } else {
                        v.vibrate(ms);
                    }
                }
            }
        } catch (Exception ignored) {}
    }

    @JavascriptInterface
    public void triggerSosHaptics() {
        // Morse SOS pattern: ... --- ...
        long[] pattern = { 0, 150, 100, 150, 100, 150, 300, 400, 100, 400, 100, 400, 300, 150, 100, 150, 100, 150 };
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                VibratorManager vm = (VibratorManager) activity.getSystemService(Context.VIBRATOR_MANAGER_SERVICE);
                if (vm != null) {
                    Vibrator v = vm.getDefaultVibrator();
                    v.vibrate(VibrationEffect.createWaveform(pattern, -1));
                }
            } else {
                Vibrator v = (Vibrator) activity.getSystemService(Context.VIBRATOR_SERVICE);
                if (v != null) {
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                        v.vibrate(VibrationEffect.createWaveform(pattern, -1));
                    } else {
                        v.vibrate(pattern, -1);
                    }
                }
            }
        } catch (Exception ignored) {}
    }

    @JavascriptInterface
    public void savePersistent(String key, String value) {
        prefs.edit().putString(key, value).apply();
    }

    @JavascriptInterface
    public String loadPersistent(String key) {
        return prefs.getString(key, "");
    }

    @JavascriptInterface
    public String getDeviceInfo() {
        return "{\"model\":\"" + Build.MANUFACTURER + " " + Build.MODEL + "\",\"android\":\"" + Build.VERSION.RELEASE + "\"}";
    }

    @JavascriptInterface
    public boolean isNativeAndroid() {
        return true;
    }
}

