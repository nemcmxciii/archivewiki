<?php
// Router for PHP's built-in server. Mirrors DokuWiki's .htaccess protections, refuses any request whose Host
// header is not loopback (DNS-rebinding guard), and requires a per-launch secret so that only the ArchiveWiki
// window (not other apps or web pages on this Mac) can use the wiki. DokuWiki's one-time password-reset link
// is the sole unauthenticated exception, so it can be opened from the user's mail app in a browser.
$host = strtolower(preg_replace('/:\d+$/', '', $_SERVER['HTTP_HOST'] ?? ''));
$uri  = $_SERVER['REQUEST_URI'] ?? '/';
$path = rawurldecode(parse_url($uri, PHP_URL_PATH) ?: '/');

function archivewiki_valid_password_reset_token($token)
{
    return is_string($token) && preg_match('/\A[a-f0-9]{32}\z/', $token) === 1;
}

function archivewiki_is_password_reset_request($path)
{
    if ($path !== '/doku.php' || !in_array($_SERVER['REQUEST_METHOD'] ?? '', ['GET', 'POST'], true)) {
        return false;
    }

    $action = $_GET['do'] ?? $_POST['do'] ?? '';
    $token = $_GET['pwauth'] ?? $_POST['pwauth'] ?? $_POST['token'] ?? '';
    return $action === 'resendpwd' && archivewiki_valid_password_reset_token($token);
}

function archivewiki_is_password_reset_asset($path, $host)
{
    if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'GET') {
        return false;
    }

    $referer = parse_url($_SERVER['HTTP_REFERER'] ?? '');
    if (!is_array($referer) || strtolower($referer['host'] ?? '') !== $host || ($referer['path'] ?? '') !== '/doku.php') {
        return false;
    }
    $request = parse_url('http://' . ($_SERVER['HTTP_HOST'] ?? ''));
    if (($referer['port'] ?? 80) !== ($request['port'] ?? 80)) {
        return false;
    }
    parse_str($referer['query'] ?? '', $referer_query);
    if (($referer_query['do'] ?? '') !== 'resendpwd' || !archivewiki_valid_password_reset_token($referer_query['pwauth'] ?? '')) {
        return false;
    }

    if ($path === '/lib/exe/fetch.php') {
        parse_str($_SERVER['QUERY_STRING'] ?? '', $asset_query);
        return in_array($asset_query['media'] ?? '', ['wiki:logo.svg', 'logo.svg', 'wiki:logo.png', 'logo.png'], true);
    }

    return in_array($path, ['/lib/exe/css.php', '/lib/exe/js.php', '/lib/exe/manifest.php'], true)
        || preg_match('#^/lib/(?:images|tpl|plugins)/.*\.(?:css|js|png|jpe?g|gif|svg|ico|webp|woff2?|ttf)$#i', $path) === 1;
}

if (!in_array($host, ['127.0.0.1', 'localhost'], true)
    || strpos($path, '..') !== false
    || preg_match('#^/(data|conf|bin|inc|vendor)(/|$)#', $path)
    || preg_match('#/\.#', $path)) {
    http_response_code(403);
    echo 'Forbidden';
    return true;
}

$password_reset_request = archivewiki_is_password_reset_request($path);
$password_reset_asset = archivewiki_is_password_reset_asset($path, $host);
$token = getenv('ARCHIVEWIKI_TOKEN');
if ($token) {
    $given  = isset($_GET['aw']) && is_string($_GET['aw']) ? $_GET['aw'] : '';
    $cookie = isset($_COOKIE['aw_session']) && is_string($_COOKIE['aw_session']) ? $_COOKIE['aw_session'] : '';
    if ($given !== '' && hash_equals($token, $given)) {
        // First request from the app: set the session cookie, then reload the same URL without the secret.
        setcookie('aw_session', $token, ['path' => '/', 'httponly' => true, 'samesite' => 'Strict']);
        $q = $_GET;
        unset($q['aw']);
        header('Location: ' . (parse_url($uri, PHP_URL_PATH) ?: '/') . ($q ? '?' . http_build_query($q) : ''));
        return true;
    }
    if (($cookie === '' || !hash_equals($token, $cookie)) && !$password_reset_request && !$password_reset_asset) {
        http_response_code(403);
        echo 'Forbidden';
        return true;
    }
}

if ($password_reset_request) {
    header('Cache-Control: no-store, private');
}

if ($path === '/_archivewiki/page-check') {
    header('Cache-Control: no-store');
    header('Content-Type: application/json; charset=utf-8');

    if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') {
        http_response_code(405);
        header('Allow: POST');
        echo json_encode(['error' => 'Use POST to check a page title.']);
        return true;
    }

    $title = $_POST['title'] ?? null;
    if (!is_string($title) || trim($title) === '' || strlen($title) > 480) {
        http_response_code(422);
        echo json_encode(['error' => 'Enter a page title up to 120 characters long.']);
        return true;
    }

    $titleLength = preg_match_all('/./us', $title);
    if ($titleLength === false || $titleLength > 120) {
        http_response_code(422);
        echo json_encode(['error' => 'Enter a page title up to 120 characters long.']);
        return true;
    }

    require_once __DIR__ . '/inc/init.php';
    $title = str_replace([':', ';', '/', '\\'], ' ', trim($title));
    $id = cleanID($title);
    if ($id === '' || strlen($id) > 220 || strpos($id, ':') !== false) {
        http_response_code(422);
        echo json_encode(['error' => 'Use a shorter title containing letters or numbers.']);
        return true;
    }

    echo json_encode([
        'id' => $id,
        'exists' => page_exists($id),
    ], JSON_UNESCAPED_UNICODE);
    return true;
}

return false; // let the built-in server handle it normally
