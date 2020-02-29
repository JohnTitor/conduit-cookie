use base64::{decode, encode};
use std::collections::HashMap;
use std::str;

use conduit::RequestExt;
use conduit_middleware::{AfterResult, BeforeResult};
use cookie::{Cookie, Key};

use super::RequestCookies;

pub struct SessionMiddleware {
    cookie_name: String,
    key: Key,
    secure: bool,
}

pub struct Session {
    pub data: HashMap<String, String>,
}

impl SessionMiddleware {
    pub fn new(cookie: &str, key: Key, secure: bool) -> SessionMiddleware {
        SessionMiddleware {
            cookie_name: cookie.to_string(),
            key,
            secure,
        }
    }

    pub fn decode(&self, cookie: Cookie<'_>) -> HashMap<String, String> {
        let mut ret = HashMap::new();
        let bytes = decode(cookie.value().as_bytes()).unwrap_or_default();
        let mut parts = bytes.split(|&a| a == 0xff);
        while let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            if key.is_empty() {
                break;
            }
            if let (Ok(key), Ok(value)) = (str::from_utf8(key), str::from_utf8(value)) {
                ret.insert(key.to_string(), value.to_string());
            }
        }
        ret
    }

    pub fn encode(&self, h: &HashMap<String, String>) -> String {
        let mut ret = Vec::new();
        for (i, (k, v)) in h.iter().enumerate() {
            if i != 0 {
                ret.push(0xff)
            }
            ret.extend(k.bytes());
            ret.push(0xff);
            ret.extend(v.bytes());
        }
        while ret.len() * 8 % 6 != 0 {
            ret.push(0xff);
        }
        encode(&ret[..])
    }
}

impl conduit_middleware::Middleware for SessionMiddleware {
    fn before(&self, req: &mut dyn RequestExt) -> BeforeResult {
        let session = {
            let jar = req.cookies_mut().signed(&self.key);
            jar.get(&self.cookie_name)
                .map(|cookie| self.decode(cookie))
                .unwrap_or_else(HashMap::new)
        };
        req.mut_extensions().insert(Session { data: session });
        Ok(())
    }

    fn after(&self, req: &mut dyn RequestExt, res: AfterResult) -> AfterResult {
        let cookie = {
            let session = req.mut_extensions().find::<Session>();
            let session = session.expect("session must be present after request");
            let encoded = self.encode(&session.data);
            Cookie::build(self.cookie_name.to_string(), encoded)
                .http_only(true)
                .secure(self.secure)
                .path("/")
                .finish()
        };
        req.cookies_mut().signed(&self.key).add(cookie);
        res
    }
}

pub trait RequestSession {
    fn session(&mut self) -> &mut HashMap<String, String>;
}

impl<T: RequestExt + ?Sized> RequestSession for T {
    fn session(&mut self) -> &mut HashMap<String, String> {
        &mut self
            .mut_extensions()
            .find_mut::<Session>()
            .expect("missing cookie session")
            .data
    }
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use conduit::{header, Body, Handler, HttpResult, Method, RequestExt, Response};
    use conduit_middleware::MiddlewareBuilder;
    use conduit_test::MockRequest;
    use cookie::{Cookie, Key};

    use {Middleware, RequestSession, SessionMiddleware};

    fn test_key() -> Key {
        let master_key: Vec<u8> = (0..32).collect();
        Key::from_master(&master_key)
    }

    #[test]
    fn simple() {
        let mut req = MockRequest::new(Method::POST, "/articles");
        let key = test_key();

        // Set the session cookie
        let mut app = MiddlewareBuilder::new(set_session);
        app.add(Middleware::new());
        app.add(SessionMiddleware::new("lol", key, false));
        let response = app.call(&mut req).ok().unwrap();

        let v = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(v.starts_with("lol"));

        // Use the session cookie
        req.header(header::COOKIE, v);
        let key = test_key();
        let mut app = MiddlewareBuilder::new(use_session);
        app.add(Middleware::new());
        app.add(SessionMiddleware::new("lol", key, false));
        assert!(app.call(&mut req).is_ok());

        fn set_session(req: &mut dyn RequestExt) -> HttpResult {
            assert!(req
                .session()
                .insert("foo".to_string(), "bar".to_string())
                .is_none());
            let body: Body = Box::new(std::io::empty());
            Response::builder().body(body)
        }
        fn use_session(req: &mut dyn RequestExt) -> HttpResult {
            assert_eq!(*req.session().get("foo").unwrap(), "bar");
            let body: Body = Box::new(std::io::empty());
            Response::builder().body(body)
        }
    }

    #[test]
    fn no_equals() {
        let key = test_key();
        let m = SessionMiddleware::new("test", key, false);
        let e = {
            let mut map = HashMap::new();
            map.insert("a".to_string(), "bc".to_string());
            m.encode(&map)
        };
        assert!(!e.ends_with("="));
        let m = m.decode(Cookie::new("foo", e));
        assert_eq!(*m.get("a").unwrap(), "bc");
    }
}
