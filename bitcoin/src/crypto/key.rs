// SPDX-License-Identifier: CC0-1.0

//! Bitcoin keys.
//!
//! This module provides keys used in Bitcoin that can be roundtrip
//! (de)serialized.

use core::fmt::{self, Write};
use core::ops;
use core::str::FromStr;

use hashes::{hash160, Hash};
use hex::FromHex;
use internals::write_err;
#[cfg(feature = "rand-std")]
pub use secp256k1::rand;
pub use secp256k1::{self, constants, Keypair, Parity, Secp256k1, Verification, XOnlyPublicKey};

use crate::crypto::ecdsa;
use crate::network::Network;
use crate::prelude::*;
use crate::taproot::{TapNodeHash, TapTweakHash};
use crate::{base58, io};

/// A Bitcoin ECDSA public key
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PublicKey {
    /// Whether this public key should be serialized as compressed
    pub compressed: bool,
    /// The actual ECDSA key
    pub inner: secp256k1::PublicKey,
}

impl PublicKey {
    /// Constructs compressed ECDSA public key from the provided generic Secp256k1 public key
    pub fn new(key: impl Into<secp256k1::PublicKey>) -> PublicKey {
        PublicKey { compressed: true, inner: key.into() }
    }

    /// Constructs uncompressed (legacy) ECDSA public key from the provided generic Secp256k1
    /// public key
    pub fn new_uncompressed(key: impl Into<secp256k1::PublicKey>) -> PublicKey {
        PublicKey { compressed: false, inner: key.into() }
    }

    fn with_serialized<R, F: FnOnce(&[u8]) -> R>(&self, f: F) -> R {
        if self.compressed {
            f(&self.inner.serialize())
        } else {
            f(&self.inner.serialize_uncompressed())
        }
    }

    /// Returns bitcoin 160-bit hash of the public key
    pub fn pubkey_hash(&self) -> PubkeyHash { self.with_serialized(PubkeyHash::hash) }

    /// Returns bitcoin 160-bit hash of the public key for witness program
    pub fn wpubkey_hash(&self) -> Option<WPubkeyHash> {
        if self.compressed {
            Some(WPubkeyHash::from_byte_array(
                hash160::Hash::hash(&self.inner.serialize()).to_byte_array(),
            ))
        } else {
            // We can't create witness pubkey hashes for an uncompressed
            // public keys
            None
        }
    }

    /// Write the public key into a writer
    pub fn write_into<W: io::Write>(&self, mut writer: W) -> Result<(), io::Error> {
        self.with_serialized(|bytes| writer.write_all(bytes))
    }

    /// Read the public key from a reader
    ///
    /// This internally reads the first byte before reading the rest, so
    /// use of a `BufReader` is recommended.
    pub fn read_from<R: io::Read>(mut reader: R) -> Result<Self, io::Error> {
        let mut bytes = [0; 65];

        reader.read_exact(&mut bytes[0..1])?;
        let bytes = if bytes[0] < 4 { &mut bytes[..33] } else { &mut bytes[..65] };

        reader.read_exact(&mut bytes[1..])?;
        Self::from_slice(bytes).map_err(|e| {
            // Need a static string for core2
            #[cfg(feature = "std")]
            let reason = e;
            #[cfg(not(feature = "std"))]
            let reason = match e {
                Error::Base58(_) => "base58 error",
                Error::Secp256k1(_) => "secp256k1 error",
                Error::InvalidKeyPrefix(_) => "invalid key prefix",
                Error::Hex(_) => "hex decoding error",
                Error::InvalidHexLength(_) => "invalid hex string length",
            };
            io::Error::new(io::ErrorKind::InvalidData, reason)
        })
    }

    /// Serialize the public key to bytes
    pub fn to_bytes(self) -> Vec<u8> {
        let mut buf = Vec::new();
        self.write_into(&mut buf).expect("vecs don't error");
        buf
    }

    /// Serialize the public key into a `SortKey`.
    ///
    /// `SortKey` is not too useful by itself, but it can be used to sort a
    /// `[PublicKey]` slice using `sort_unstable_by_key`, `sort_by_cached_key`,
    /// `sort_by_key`, or any of the other `*_by_key` methods on slice.
    /// Pass the method into the sort method directly. (ie. `PublicKey::to_sort_key`)
    ///
    /// This method of sorting is in line with Bitcoin Core's implementation of
    /// sorting keys for output descriptors such as `sortedmulti()`.
    ///
    /// If every `PublicKey` in the slice is `compressed == true` then this will sort
    /// the keys in a
    /// [BIP67](https://github.com/bitcoin/bips/blob/master/bip-0067.mediawiki)
    /// compliant way.
    ///
    /// # Example: Using with `sort_unstable_by_key`
    ///
    /// ```rust
    /// use std::str::FromStr;
    /// use bitcoin::PublicKey;
    ///
    /// let pk = |s| PublicKey::from_str(s).unwrap();
    ///
    /// let mut unsorted = [
    ///     pk("04c4b0bbb339aa236bff38dbe6a451e111972a7909a126bc424013cba2ec33bc38e98ac269ffe028345c31ac8d0a365f29c8f7e7cfccac72f84e1acd02bc554f35"),
    ///     pk("038f47dcd43ba6d97fc9ed2e3bba09b175a45fac55f0683e8cf771e8ced4572354"),
    ///     pk("028bde91b10013e08949a318018fedbd896534a549a278e220169ee2a36517c7aa"),
    ///     pk("04c4b0bbb339aa236bff38dbe6a451e111972a7909a126bc424013cba2ec33bc3816753d96001fd7cba3ce5372f5c9a0d63708183033538d07b1e532fc43aaacfa"),
    ///     pk("032b8324c93575034047a52e9bca05a46d8347046b91a032eff07d5de8d3f2730b"),
    ///     pk("045d753414fa292ea5b8f56e39cfb6a0287b2546231a5cb05c4b14ab4b463d171f5128148985b23eccb1e2905374873b1f09b9487f47afa6b1f2b0083ac8b4f7e8"),
    ///     pk("0234dd69c56c36a41230d573d68adeae0030c9bc0bf26f24d3e1b64c604d293c68"),
    /// ];
    /// let sorted = [
    ///     // These first 4 keys are in a BIP67 compatible sorted order
    ///     // (since they are compressed)
    ///     pk("0234dd69c56c36a41230d573d68adeae0030c9bc0bf26f24d3e1b64c604d293c68"),
    ///     pk("028bde91b10013e08949a318018fedbd896534a549a278e220169ee2a36517c7aa"),
    ///     pk("032b8324c93575034047a52e9bca05a46d8347046b91a032eff07d5de8d3f2730b"),
    ///     pk("038f47dcd43ba6d97fc9ed2e3bba09b175a45fac55f0683e8cf771e8ced4572354"),
    ///     // Uncompressed keys are not BIP67 compliant, but are sorted
    ///     // after compressed keys in Bitcoin Core using `sortedmulti()`
    ///     pk("045d753414fa292ea5b8f56e39cfb6a0287b2546231a5cb05c4b14ab4b463d171f5128148985b23eccb1e2905374873b1f09b9487f47afa6b1f2b0083ac8b4f7e8"),
    ///     pk("04c4b0bbb339aa236bff38dbe6a451e111972a7909a126bc424013cba2ec33bc3816753d96001fd7cba3ce5372f5c9a0d63708183033538d07b1e532fc43aaacfa"),
    ///     pk("04c4b0bbb339aa236bff38dbe6a451e111972a7909a126bc424013cba2ec33bc38e98ac269ffe028345c31ac8d0a365f29c8f7e7cfccac72f84e1acd02bc554f35"),
    /// ];
    ///
    /// unsorted.sort_unstable_by_key(|k| PublicKey::to_sort_key(*k));
    ///
    /// assert_eq!(unsorted, sorted);
    /// ```
    pub fn to_sort_key(self) -> SortKey {
        if self.compressed {
            let bytes = self.inner.serialize();
            let mut res = [0; 32];
            res[..].copy_from_slice(&bytes[1..33]);
            SortKey(bytes[0], res, [0; 32])
        } else {
            let bytes = self.inner.serialize_uncompressed();
            let mut res_left = [0; 32];
            let mut res_right = [0; 32];
            res_left[..].copy_from_slice(&bytes[1..33]);
            res_right[..].copy_from_slice(&bytes[33..65]);
            SortKey(bytes[0], res_left, res_right)
        }
    }

    /// Deserialize a public key from a slice
    pub fn from_slice(data: &[u8]) -> Result<PublicKey, Error> {
        let compressed = match data.len() {
            33 => true,
            65 => false,
            len => {
                return Err(base58::Error::InvalidLength(len).into());
            }
        };

        if !compressed && data[0] != 0x04 {
            return Err(Error::InvalidKeyPrefix(data[0]));
        }

        Ok(PublicKey { compressed, inner: secp256k1::PublicKey::from_slice(data)? })
    }

    /// Computes the public key as supposed to be used with this secret
    pub fn from_private_key<C: secp256k1::Signing>(
        secp: &Secp256k1<C>,
        sk: &PrivateKey,
    ) -> PublicKey {
        sk.public_key(secp)
    }

    /// Checks that `sig` is a valid ECDSA signature for `msg` using this public key.
    pub fn verify<C: secp256k1::Verification>(
        &self,
        secp: &Secp256k1<C>,
        msg: &secp256k1::Message,
        sig: &ecdsa::Signature,
    ) -> Result<(), Error> {
        Ok(secp.verify_ecdsa(msg, &sig.sig, &self.inner)?)
    }
}

impl From<secp256k1::PublicKey> for PublicKey {
    fn from(pk: secp256k1::PublicKey) -> PublicKey { PublicKey::new(pk) }
}

impl From<PublicKey> for XOnlyPublicKey {
    fn from(pk: PublicKey) -> XOnlyPublicKey { pk.inner.into() }
}

/// An opaque return type for PublicKey::to_sort_key
#[derive(Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub struct SortKey(u8, [u8; 32], [u8; 32]);

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // TODO: fast hex encoding
        self.with_serialized(|bytes| {
            for ch in bytes {
                write!(f, "{:02x}", ch)?;
            }
            Ok(())
        })
    }
}

impl FromStr for PublicKey {
    type Err = Error;
    fn from_str(s: &str) -> Result<PublicKey, Error> {
        match s.len() {
            66 => PublicKey::from_slice(&<[u8; 33]>::from_hex(s)?),
            130 => PublicKey::from_slice(&<[u8; 65]>::from_hex(s)?),
            len => Err(Error::InvalidHexLength(len)),
        }
    }
}

hashes::hash_newtype! {
    /// A hash of a public key.
    pub struct PubkeyHash(hash160::Hash);
    /// SegWit version of a public key hash.
    pub struct WPubkeyHash(hash160::Hash);
}
crate::hash_types::impl_asref_push_bytes!(PubkeyHash, WPubkeyHash);

impl From<PublicKey> for PubkeyHash {
    fn from(key: PublicKey) -> PubkeyHash { key.pubkey_hash() }
}

impl From<&PublicKey> for PubkeyHash {
    fn from(key: &PublicKey) -> PubkeyHash { key.pubkey_hash() }
}

/// A Bitcoin ECDSA private key
#[derive(Copy, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct PrivateKey {
    /// Whether this private key should be serialized as compressed
    pub compressed: bool,
    /// The network on which this key should be used
    pub network: Network,
    /// The actual ECDSA key
    pub inner: secp256k1::SecretKey,
}

impl PrivateKey {
    /// Constructs new compressed ECDSA private key using the secp256k1 algorithm and
    /// a secure random number generator.
    #[cfg(feature = "rand-std")]
    pub fn generate(network: Network) -> PrivateKey {
        let secret_key = secp256k1::SecretKey::new(&mut rand::thread_rng());
        PrivateKey::new(secret_key, network)
    }
    /// Constructs compressed ECDSA private key from the provided generic Secp256k1 private key
    /// and the specified network
    pub fn new(key: secp256k1::SecretKey, network: Network) -> PrivateKey {
        PrivateKey { compressed: true, network, inner: key }
    }

    /// Constructs uncompressed (legacy) ECDSA private key from the provided generic Secp256k1
    /// private key and the specified network
    pub fn new_uncompressed(key: secp256k1::SecretKey, network: Network) -> PrivateKey {
        PrivateKey { compressed: false, network, inner: key }
    }

    /// Creates a public key from this private key
    pub fn public_key<C: secp256k1::Signing>(&self, secp: &Secp256k1<C>) -> PublicKey {
        PublicKey {
            compressed: self.compressed,
            inner: secp256k1::PublicKey::from_secret_key(secp, &self.inner),
        }
    }

    /// Serialize the private key to bytes
    pub fn to_bytes(self) -> Vec<u8> { self.inner[..].to_vec() }

    /// Deserialize a private key from a slice
    pub fn from_slice(data: &[u8], network: Network) -> Result<PrivateKey, Error> {
        Ok(PrivateKey::new(secp256k1::SecretKey::from_slice(data)?, network))
    }

    /// Format the private key to WIF format.
    pub fn fmt_wif(&self, fmt: &mut dyn fmt::Write) -> fmt::Result {
        let mut ret = [0; 34];
        ret[0] = match self.network {
            Network::Bitcoin => 213,
            Network::Testnet => 153,
            Network::Signet | Network::Regtest => 153,
        };
        ret[1..33].copy_from_slice(&self.inner[..]);
        let privkey = if self.compressed {
            ret[33] = 1;
            base58::encode_check(&ret[..])
        } else {
            base58::encode_check(&ret[..33])
        };
        fmt.write_str(&privkey)
    }

    /// Get WIF encoding of this private key.
    pub fn to_wif(self) -> String {
        let mut buf = String::new();
        buf.write_fmt(format_args!("{}", self)).unwrap();
        buf.shrink_to_fit();
        buf
    }

    /// Parse WIF encoded private key.
    pub fn from_wif(wif: &str) -> Result<PrivateKey, Error> {
        let data = base58::decode_check(wif)?;

        let compressed = match data.len() {
            33 => false,
            34 => true,
            _ => {
                return Err(Error::Base58(base58::Error::InvalidLength(data.len())));
            }
        };

        let network = match data[0] {
            176 => Network::Bitcoin,
            176 => Network::Regtest,
            176 => Network::Testnet,
            x => {
                return Err(Error::Base58(base58::Error::InvalidAddressVersion(x)));
            }
        };

        Ok(PrivateKey {
            compressed,
            network,
            inner: secp256k1::SecretKey::from_slice(&data[1..33])?,
        })
    }
}

impl fmt::Display for PrivateKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { self.fmt_wif(f) }
}

#[cfg(not(feature = "std"))]
impl fmt::Debug for PrivateKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { write!(f, "[private key data]") }
}

impl FromStr for PrivateKey {
    type Err = Error;
    fn from_str(s: &str) -> Result<PrivateKey, Error> { PrivateKey::from_wif(s) }
}

impl ops::Index<ops::RangeFull> for PrivateKey {
    type Output = [u8];
    fn index(&self, _: ops::RangeFull) -> &[u8] { &self.inner[..] }
}

#[cfg(feature = "serde")]
impl serde::Serialize for PrivateKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for PrivateKey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<PrivateKey, D::Error> {
        struct WifVisitor;

        impl<'de> serde::de::Visitor<'de> for WifVisitor {
            type Value = PrivateKey;

            fn expecting(&self, formatter: &mut core::fmt::Formatter) -> core::fmt::Result {
                formatter.write_str("an ASCII WIF string")
            }

            fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if let Ok(s) = core::str::from_utf8(v) {
                    PrivateKey::from_str(s).map_err(E::custom)
                } else {
                    Err(E::invalid_value(::serde::de::Unexpected::Bytes(v), &self))
                }
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                PrivateKey::from_str(v).map_err(E::custom)
            }
        }

        d.deserialize_str(WifVisitor)
    }
}

#[cfg(feature = "serde")]
#[allow(clippy::collapsible_else_if)] // Aids readability.
impl serde::Serialize for PublicKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() {
            s.collect_str(self)
        } else {
            self.with_serialized(|bytes| s.serialize_bytes(bytes))
        }
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for PublicKey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<PublicKey, D::Error> {
        if d.is_human_readable() {
            struct HexVisitor;

            impl<'de> serde::de::Visitor<'de> for HexVisitor {
                type Value = PublicKey;

                fn expecting(&self, formatter: &mut core::fmt::Formatter) -> core::fmt::Result {
                    formatter.write_str("an ASCII hex string")
                }

                fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
                where
                    E: serde::de::Error,
                {
                    if let Ok(hex) = core::str::from_utf8(v) {
                        PublicKey::from_str(hex).map_err(E::custom)
                    } else {
                        Err(E::invalid_value(::serde::de::Unexpected::Bytes(v), &self))
                    }
                }

                fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
                where
                    E: serde::de::Error,
                {
                    PublicKey::from_str(v).map_err(E::custom)
                }
            }
            d.deserialize_str(HexVisitor)
        } else {
            struct BytesVisitor;

            impl<'de> serde::de::Visitor<'de> for BytesVisitor {
                type Value = PublicKey;

                fn expecting(&self, formatter: &mut core::fmt::Formatter) -> core::fmt::Result {
                    formatter.write_str("a bytestring")
                }

                fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
                where
                    E: serde::de::Error,
                {
                    PublicKey::from_slice(v).map_err(E::custom)
                }
            }

            d.deserialize_bytes(BytesVisitor)
        }
    }
}

/// Untweaked BIP-340 X-coord-only public key
pub type UntweakedPublicKey = XOnlyPublicKey;

/// Tweaked BIP-340 X-coord-only public key
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(crate = "actual_serde"))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct TweakedPublicKey(XOnlyPublicKey);

impl fmt::LowerHex for TweakedPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { fmt::LowerHex::fmt(&self.0, f) }
}

impl fmt::Display for TweakedPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { fmt::Display::fmt(&self.0, f) }
}

/// Untweaked BIP-340 key pair
#[deprecated(since = "0.31.0", note = "use UntweakedKeypair instead")]
#[allow(deprecated)]
pub type UntweakedKeyPair = UntweakedKeypair;

/// Untweaked BIP-340 key pair
pub type UntweakedKeypair = Keypair;

/// Tweaked BIP-340 key pair
#[deprecated(since = "0.31.0", note = "use TweakedKeypair instead")]
#[allow(deprecated)]
pub type TweakedKeyPair = TweakedKeypair;

/// Tweaked BIP-340 key pair
///
/// # Examples
/// ```
/// # #[cfg(feature = "rand-std")] {
/// # use bitcoin::key::{Keypair, TweakedKeypair, TweakedPublicKey};
/// # use bitcoin::secp256k1::{rand, Secp256k1};
/// # let secp = Secp256k1::new();
/// # let keypair = TweakedKeypair::dangerous_assume_tweaked(Keypair::new(&secp, &mut rand::thread_rng()));
/// // There are various conversion methods available to get a tweaked pubkey from a tweaked keypair.
/// let (_pk, _parity) = keypair.public_parts();
/// let _pk  = TweakedPublicKey::from_keypair(keypair);
/// let _pk = TweakedPublicKey::from(keypair);
/// # }
/// ```
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(crate = "actual_serde"))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct TweakedKeypair(Keypair);

/// A trait for tweaking BIP340 key types (x-only public keys and key pairs).
pub trait TapTweak {
    /// Tweaked key type with optional auxiliary information
    type TweakedAux;
    /// Tweaked key type
    type TweakedKey;

    /// Tweaks an untweaked key with corresponding public key value and optional script tree merkle
    /// root. For the [`Keypair`] type this also tweaks the private key in the pair.
    ///
    /// This is done by using the equation Q = P + H(P|c)G, where
    ///  * Q is the tweaked public key
    ///  * P is the internal public key
    ///  * H is the hash function
    ///  * c is the commitment data
    ///  * G is the generator point
    ///
    /// # Returns
    /// The tweaked key and its parity.
    fn tap_tweak<C: Verification>(
        self,
        secp: &Secp256k1<C>,
        merkle_root: Option<TapNodeHash>,
    ) -> Self::TweakedAux;

    /// Directly converts an [`UntweakedPublicKey`] to a [`TweakedPublicKey`]
    ///
    /// This method is dangerous and can lead to loss of funds if used incorrectly.
    /// Specifically, in multi-party protocols a peer can provide a value that allows them to steal.
    fn dangerous_assume_tweaked(self) -> Self::TweakedKey;
}

impl TapTweak for UntweakedPublicKey {
    type TweakedAux = (TweakedPublicKey, Parity);
    type TweakedKey = TweakedPublicKey;

    /// Tweaks an untweaked public key with corresponding public key value and optional script tree
    /// merkle root.
    ///
    /// This is done by using the equation Q = P + H(P|c)G, where
    ///  * Q is the tweaked public key
    ///  * P is the internal public key
    ///  * H is the hash function
    ///  * c is the commitment data
    ///  * G is the generator point
    ///
    /// # Returns
    /// The tweaked key and its parity.
    fn tap_tweak<C: Verification>(
        self,
        secp: &Secp256k1<C>,
        merkle_root: Option<TapNodeHash>,
    ) -> (TweakedPublicKey, Parity) {
        let tweak = TapTweakHash::from_key_and_tweak(self, merkle_root).to_scalar();
        let (output_key, parity) = self.add_tweak(secp, &tweak).expect("Tap tweak failed");

        debug_assert!(self.tweak_add_check(secp, &output_key, parity, tweak));
        (TweakedPublicKey(output_key), parity)
    }

    fn dangerous_assume_tweaked(self) -> TweakedPublicKey { TweakedPublicKey(self) }
}

impl TapTweak for UntweakedKeypair {
    type TweakedAux = TweakedKeypair;
    type TweakedKey = TweakedKeypair;

    /// Tweaks private and public keys within an untweaked [`Keypair`] with corresponding public key
    /// value and optional script tree merkle root.
    ///
    /// This is done by tweaking private key within the pair using the equation q = p + H(P|c), where
    ///  * q is the tweaked private key
    ///  * p is the internal private key
    ///  * H is the hash function
    ///  * c is the commitment data
    /// The public key is generated from a private key by multiplying with generator point, Q = qG.
    ///
    /// # Returns
    /// The tweaked key and its parity.
    fn tap_tweak<C: Verification>(
        self,
        secp: &Secp256k1<C>,
        merkle_root: Option<TapNodeHash>,
    ) -> TweakedKeypair {
        let (pubkey, _parity) = XOnlyPublicKey::from_keypair(&self);
        let tweak = TapTweakHash::from_key_and_tweak(pubkey, merkle_root).to_scalar();
        let tweaked = self.add_xonly_tweak(secp, &tweak).expect("Tap tweak failed");
        TweakedKeypair(tweaked)
    }

    fn dangerous_assume_tweaked(self) -> TweakedKeypair { TweakedKeypair(self) }
}

impl TweakedPublicKey {
    /// Returns the [`TweakedPublicKey`] for `keypair`.
    #[inline]
    pub fn from_keypair(keypair: TweakedKeypair) -> Self {
        let (xonly, _parity) = keypair.0.x_only_public_key();
        TweakedPublicKey(xonly)
    }

    /// Creates a new [`TweakedPublicKey`] from a [`XOnlyPublicKey`]. No tweak is applied, consider
    /// calling `tap_tweak` on an [`UntweakedPublicKey`] instead of using this constructor.
    ///
    /// This method is dangerous and can lead to loss of funds if used incorrectly.
    /// Specifically, in multi-party protocols a peer can provide a value that allows them to steal.
    #[inline]
    pub fn dangerous_assume_tweaked(key: XOnlyPublicKey) -> TweakedPublicKey {
        TweakedPublicKey(key)
    }

    /// Returns the underlying public key.
    pub fn to_inner(self) -> XOnlyPublicKey { self.0 }

    /// Serialize the key as a byte-encoded pair of values. In compressed form
    /// the y-coordinate is represented by only a single bit, as x determines
    /// it up to one bit.
    #[inline]
    pub fn serialize(&self) -> [u8; constants::SCHNORR_PUBLIC_KEY_SIZE] { self.0.serialize() }
}

impl TweakedKeypair {
    /// Creates a new [`TweakedKeypair`] from a [`Keypair`]. No tweak is applied, consider
    /// calling `tap_tweak` on an [`UntweakedKeypair`] instead of using this constructor.
    ///
    /// This method is dangerous and can lead to loss of funds if used incorrectly.
    /// Specifically, in multi-party protocols a peer can provide a value that allows them to steal.
    #[inline]
    pub fn dangerous_assume_tweaked(pair: Keypair) -> TweakedKeypair { TweakedKeypair(pair) }

    /// Returns the underlying key pair.
    #[inline]
    pub fn to_inner(self) -> Keypair { self.0 }

    /// Returns the [`TweakedPublicKey`] and its [`Parity`] for this [`TweakedKeypair`].
    #[inline]
    pub fn public_parts(&self) -> (TweakedPublicKey, Parity) {
        let (xonly, parity) = self.0.x_only_public_key();
        (TweakedPublicKey(xonly), parity)
    }
}

impl From<TweakedPublicKey> for XOnlyPublicKey {
    #[inline]
    fn from(pair: TweakedPublicKey) -> Self { pair.0 }
}

impl From<TweakedKeypair> for Keypair {
    #[inline]
    fn from(pair: TweakedKeypair) -> Self { pair.0 }
}

impl From<TweakedKeypair> for TweakedPublicKey {
    #[inline]
    fn from(pair: TweakedKeypair) -> Self { TweakedPublicKey::from_keypair(pair) }
}
/// A key-related error.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// A base58 error.
    Base58(base58::Error),
    /// A secp256k1 error.
    Secp256k1(secp256k1::Error),
    /// Invalid key prefix error.
    InvalidKeyPrefix(u8),
    /// Hex decoding error.
    Hex(hex::HexToArrayError),
    /// `PublicKey` hex should be 66 or 130 digits long.
    InvalidHexLength(usize),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use Error::*;

        match *self {
            Base58(ref e) => write_err!(f, "base58"; e),
            Secp256k1(ref e) => write_err!(f, "secp256k1"; e),
            InvalidKeyPrefix(ref b) => write!(f, "key prefix invalid: {}", b),
            Hex(ref e) => write_err!(f, "hex"; e),
            InvalidHexLength(got) =>
                write!(f, "pubkey hex should be 66 or 130 digits long, got: {}", got),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        use Error::*;

        match *self {
            Base58(ref e) => Some(e),
            Secp256k1(ref e) => Some(e),
            Hex(ref e) => Some(e),
            InvalidKeyPrefix(_) | InvalidHexLength(_) => None,
        }
    }
}

impl From<base58::Error> for Error {
    fn from(e: base58::Error) -> Error { Error::Base58(e) }
}

impl From<secp256k1::Error> for Error {
    fn from(e: secp256k1::Error) -> Error { Error::Secp256k1(e) }
}

impl From<hex::HexToArrayError> for Error {
    fn from(e: hex::HexToArrayError) -> Self { Error::Hex(e) }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use hex::FromHex;
    use secp256k1::Secp256k1;

    use super::*;
    use crate::address::Address;
    use crate::io;
    use crate::network::Network::{Bitcoin, Testnet};

    #[test]
    fn test_key_derivation() {
        // testnet compressed
        let sk =
            PrivateKey::from_wif("cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy").unwrap();
        assert_eq!(sk.network, Testnet);
        assert!(sk.compressed);
        assert_eq!(&sk.to_wif(), "cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy");

        let secp = Secp256k1::new();
        let pk = Address::p2pkh(&sk.public_key(&secp), sk.network);
        assert_eq!(&pk.to_string(), "mqwpxxvfv3QbM8PU8uBx2jaNt9btQqvQNx");

        // test string conversion
        assert_eq!(&sk.to_string(), "cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy");
        let sk_str =
            PrivateKey::from_str("cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy").unwrap();
        assert_eq!(&sk.to_wif(), &sk_str.to_wif());

        // mainnet uncompressed
        let sk =
            PrivateKey::from_wif("5JYkZjmN7PVMjJUfJWfRFwtuXTGB439XV6faajeHPAM9Z2PT2R3").unwrap();
        assert_eq!(sk.network, Bitcoin);
        assert!(!sk.compressed);
        assert_eq!(&sk.to_wif(), "5JYkZjmN7PVMjJUfJWfRFwtuXTGB439XV6faajeHPAM9Z2PT2R3");

        let secp = Secp256k1::new();
        let mut pk = sk.public_key(&secp);
        assert!(!pk.compressed);
        assert_eq!(&pk.to_string(), "042e58afe51f9ed8ad3cc7897f634d881fdbe49a81564629ded8156bebd2ffd1af191923a2964c177f5b5923ae500fca49e99492d534aa3759d6b25a8bc971b133");
        assert_eq!(pk, PublicKey::from_str("042e58afe51f9ed8ad3cc7897f634d881fdbe49a81564629ded8156bebd2ffd1af191923a2964c177f5b5923ae500fca49e99492d534aa3759d6b25a8bc971b133").unwrap());
        let addr = Address::p2pkh(&pk, sk.network);
        assert_eq!(&addr.to_string(), "1GhQvF6dL8xa6wBxLnWmHcQsurx9RxiMc8");
        pk.compressed = true;
        assert_eq!(
            &pk.to_string(),
            "032e58afe51f9ed8ad3cc7897f634d881fdbe49a81564629ded8156bebd2ffd1af"
        );
        assert_eq!(
            pk,
            PublicKey::from_str(
                "032e58afe51f9ed8ad3cc7897f634d881fdbe49a81564629ded8156bebd2ffd1af"
            )
            .unwrap()
        );
    }

    #[test]
    fn test_pubkey_hash() {
        let pk = PublicKey::from_str(
            "032e58afe51f9ed8ad3cc7897f634d881fdbe49a81564629ded8156bebd2ffd1af",
        )
        .unwrap();
        let upk = PublicKey::from_str("042e58afe51f9ed8ad3cc7897f634d881fdbe49a81564629ded8156bebd2ffd1af191923a2964c177f5b5923ae500fca49e99492d534aa3759d6b25a8bc971b133").unwrap();
        assert_eq!(pk.pubkey_hash().to_string(), "9511aa27ef39bbfa4e4f3dd15f4d66ea57f475b4");
        assert_eq!(upk.pubkey_hash().to_string(), "ac2e7daf42d2c97418fd9f78af2de552bb9c6a7a");
    }

    #[test]
    fn test_wpubkey_hash() {
        let pk = PublicKey::from_str(
            "032e58afe51f9ed8ad3cc7897f634d881fdbe49a81564629ded8156bebd2ffd1af",
        )
        .unwrap();
        let upk = PublicKey::from_str("042e58afe51f9ed8ad3cc7897f634d881fdbe49a81564629ded8156bebd2ffd1af191923a2964c177f5b5923ae500fca49e99492d534aa3759d6b25a8bc971b133").unwrap();
        assert_eq!(
            pk.wpubkey_hash().unwrap().to_string(),
            "9511aa27ef39bbfa4e4f3dd15f4d66ea57f475b4"
        );
        assert_eq!(upk.wpubkey_hash(), None);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_key_serde() {
        use serde_test::{assert_tokens, Configure, Token};

        static KEY_WIF: &str = "cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy";
        static PK_STR: &str = "039b6347398505f5ec93826dc61c19f47c66c0283ee9be980e29ce325a0f4679ef";
        static PK_STR_U: &str = "\
            04\
            9b6347398505f5ec93826dc61c19f47c66c0283ee9be980e29ce325a0f4679ef\
            87288ed73ce47fc4f5c79d19ebfa57da7cff3aff6e819e4ee971d86b5e61875d\
        ";
        #[rustfmt::skip]
        static PK_BYTES: [u8; 33] = [
            0x03,
            0x9b, 0x63, 0x47, 0x39, 0x85, 0x05, 0xf5, 0xec,
            0x93, 0x82, 0x6d, 0xc6, 0x1c, 0x19, 0xf4, 0x7c,
            0x66, 0xc0, 0x28, 0x3e, 0xe9, 0xbe, 0x98, 0x0e,
            0x29, 0xce, 0x32, 0x5a, 0x0f, 0x46, 0x79, 0xef,
        ];
        #[rustfmt::skip]
        static PK_BYTES_U: [u8; 65] = [
            0x04,
            0x9b, 0x63, 0x47, 0x39, 0x85, 0x05, 0xf5, 0xec,
            0x93, 0x82, 0x6d, 0xc6, 0x1c, 0x19, 0xf4, 0x7c,
            0x66, 0xc0, 0x28, 0x3e, 0xe9, 0xbe, 0x98, 0x0e,
            0x29, 0xce, 0x32, 0x5a, 0x0f, 0x46, 0x79, 0xef,
            0x87, 0x28, 0x8e, 0xd7, 0x3c, 0xe4, 0x7f, 0xc4,
            0xf5, 0xc7, 0x9d, 0x19, 0xeb, 0xfa, 0x57, 0xda,
            0x7c, 0xff, 0x3a, 0xff, 0x6e, 0x81, 0x9e, 0x4e,
            0xe9, 0x71, 0xd8, 0x6b, 0x5e, 0x61, 0x87, 0x5d,
        ];

        let s = Secp256k1::new();
        let sk = PrivateKey::from_str(KEY_WIF).unwrap();
        let pk = PublicKey::from_private_key(&s, &sk);
        let pk_u = PublicKey { inner: pk.inner, compressed: false };

        assert_tokens(&sk, &[Token::BorrowedStr(KEY_WIF)]);
        assert_tokens(&pk.compact(), &[Token::BorrowedBytes(&PK_BYTES[..])]);
        assert_tokens(&pk.readable(), &[Token::BorrowedStr(PK_STR)]);
        assert_tokens(&pk_u.compact(), &[Token::BorrowedBytes(&PK_BYTES_U[..])]);
        assert_tokens(&pk_u.readable(), &[Token::BorrowedStr(PK_STR_U)]);
    }

    fn random_key(mut seed: u8) -> PublicKey {
        loop {
            let mut data = [0; 65];
            for byte in &mut data[..] {
                *byte = seed;
                // totally a rng
                seed = seed.wrapping_mul(41).wrapping_add(43);
            }
            if data[0] % 2 == 0 {
                data[0] = 4;
                if let Ok(key) = PublicKey::from_slice(&data[..]) {
                    return key;
                }
            } else {
                data[0] = 2 + (data[0] >> 7);
                if let Ok(key) = PublicKey::from_slice(&data[..33]) {
                    return key;
                }
            }
        }
    }

    #[test]
    fn pubkey_read_write() {
        const N_KEYS: usize = 20;
        let keys: Vec<_> = (0..N_KEYS).map(|i| random_key(i as u8)).collect();

        let mut v = vec![];
        for k in &keys {
            k.write_into(&mut v).expect("writing into vec");
        }

        let mut dec_keys = vec![];
        let mut cursor = io::Cursor::new(&v);
        for _ in 0..N_KEYS {
            dec_keys.push(PublicKey::read_from(&mut cursor).expect("reading from vec"));
        }

        assert_eq!(keys, dec_keys);

        // sanity checks
        assert!(PublicKey::read_from(&mut cursor).is_err());
        assert!(PublicKey::read_from(io::Cursor::new(&[])).is_err());
        assert!(PublicKey::read_from(io::Cursor::new(&[0; 33][..])).is_err());
        assert!(PublicKey::read_from(io::Cursor::new(&[2; 32][..])).is_err());
        assert!(PublicKey::read_from(io::Cursor::new(&[0; 65][..])).is_err());
        assert!(PublicKey::read_from(io::Cursor::new(&[4; 64][..])).is_err());
    }

    #[test]
    fn pubkey_to_sort_key() {
        let key1 = PublicKey::from_str(
            "02ff12471208c14bd580709cb2358d98975247d8765f92bc25eab3b2763ed605f8",
        )
        .unwrap();
        let key2 = PublicKey { inner: key1.inner, compressed: false };
        let expected1 = SortKey(
            2,
            <[u8; 32]>::from_hex(
                "ff12471208c14bd580709cb2358d98975247d8765f92bc25eab3b2763ed605f8",
            )
            .unwrap(),
            [0_u8; 32],
        );
        let expected2 = SortKey(
            4,
            <[u8; 32]>::from_hex(
                "ff12471208c14bd580709cb2358d98975247d8765f92bc25eab3b2763ed605f8",
            )
            .unwrap(),
            <[u8; 32]>::from_hex(
                "1794e7f3d5e420641a3bc690067df5541470c966cbca8c694bf39aa16d836918",
            )
            .unwrap(),
        );
        assert_eq!(key1.to_sort_key(), expected1);
        assert_eq!(key2.to_sort_key(), expected2);
    }

    #[test]
    fn pubkey_sort() {
        struct Vector {
            input: Vec<PublicKey>,
            expect: Vec<PublicKey>,
        }
        let fmt =
            |v: Vec<_>| v.into_iter().map(|s| PublicKey::from_str(s).unwrap()).collect::<Vec<_>>();
        let vectors = vec![
            // Start BIP67 vectors
            // Vector 1
            Vector {
                input: fmt(vec![
                    "02ff12471208c14bd580709cb2358d98975247d8765f92bc25eab3b2763ed605f8",
                    "02fe6f0a5a297eb38c391581c4413e084773ea23954d93f7753db7dc0adc188b2f",
                ]),
                expect: fmt(vec![
                    "02fe6f0a5a297eb38c391581c4413e084773ea23954d93f7753db7dc0adc188b2f",
                    "02ff12471208c14bd580709cb2358d98975247d8765f92bc25eab3b2763ed605f8",
                ]),
            },
            // Vector 2 (Already sorted, no action required)
            Vector {
                input: fmt(vec![
                    "02632b12f4ac5b1d1b72b2a3b508c19172de44f6f46bcee50ba33f3f9291e47ed0",
                    "027735a29bae7780a9755fae7a1c4374c656ac6a69ea9f3697fda61bb99a4f3e77",
                    "02e2cc6bd5f45edd43bebe7cb9b675f0ce9ed3efe613b177588290ad188d11b404",
                ]),
                expect: fmt(vec![
                    "02632b12f4ac5b1d1b72b2a3b508c19172de44f6f46bcee50ba33f3f9291e47ed0",
                    "027735a29bae7780a9755fae7a1c4374c656ac6a69ea9f3697fda61bb99a4f3e77",
                    "02e2cc6bd5f45edd43bebe7cb9b675f0ce9ed3efe613b177588290ad188d11b404",
                ]),
            },
            // Vector 3
            Vector {
                input: fmt(vec![
                    "030000000000000000000000000000000000004141414141414141414141414141",
                    "020000000000000000000000000000000000004141414141414141414141414141",
                    "020000000000000000000000000000000000004141414141414141414141414140",
                    "030000000000000000000000000000000000004141414141414141414141414140",
                ]),
                expect: fmt(vec![
                    "020000000000000000000000000000000000004141414141414141414141414140",
                    "020000000000000000000000000000000000004141414141414141414141414141",
                    "030000000000000000000000000000000000004141414141414141414141414140",
                    "030000000000000000000000000000000000004141414141414141414141414141",
                ]),
            },
            // Vector 4: (from bitcore)
            Vector {
                input: fmt(vec![
                    "022df8750480ad5b26950b25c7ba79d3e37d75f640f8e5d9bcd5b150a0f85014da",
                    "03e3818b65bcc73a7d64064106a859cc1a5a728c4345ff0b641209fba0d90de6e9",
                    "021f2f6e1e50cb6a953935c3601284925decd3fd21bc445712576873fb8c6ebc18",
                ]),
                expect: fmt(vec![
                    "021f2f6e1e50cb6a953935c3601284925decd3fd21bc445712576873fb8c6ebc18",
                    "022df8750480ad5b26950b25c7ba79d3e37d75f640f8e5d9bcd5b150a0f85014da",
                    "03e3818b65bcc73a7d64064106a859cc1a5a728c4345ff0b641209fba0d90de6e9",
                ]),
            },
            // Non-BIP67 vectors
            Vector {
                input: fmt(vec![
                    "02c690d642c1310f3a1ababad94e3930e4023c930ea472e7f37f660fe485263b88",
                    "0234dd69c56c36a41230d573d68adeae0030c9bc0bf26f24d3e1b64c604d293c68",
                    "041a181bd0e79974bd7ca552e09fc42ba9c3d5dbb3753741d6f0ab3015dbfd9a22d6b001a32f5f51ac6f2c0f35e73a6a62f59e848fa854d3d21f3f231594eeaa46",
                    "032b8324c93575034047a52e9bca05a46d8347046b91a032eff07d5de8d3f2730b",
                    "04c4b0bbb339aa236bff38dbe6a451e111972a7909a126bc424013cba2ec33bc3816753d96001fd7cba3ce5372f5c9a0d63708183033538d07b1e532fc43aaacfa",
                    "028e1c947c8c0b8ed021088b8e981491ac7af2b8fabebea1abdb448424c8ed75b7",
                    "045d753414fa292ea5b8f56e39cfb6a0287b2546231a5cb05c4b14ab4b463d171f5128148985b23eccb1e2905374873b1f09b9487f47afa6b1f2b0083ac8b4f7e8",
                    "03004a8a3d242d7957c0b60fb7208d386fa6a0193aabd1f3f095ffd0ac097e447b",
                    "04eb0db2d71ccbb0edd8fb35092cbcae2f7fa1f06d4c170804bf52007924b569a8d2d6f6bc8fd2b3caa3253fa1bb674443743bf7fb9f94f9c0b0831a252894cfa8",
                    "04516cde23e14f2319423b7a4a7ae48b1dadceb5e9c123198d417d10895684c42eb05e210f90ccbc72448803a22312e3f122ff2939956ccef4f7316f836295ddd5",
                    "038f47dcd43ba6d97fc9ed2e3bba09b175a45fac55f0683e8cf771e8ced4572354",
                    "04c6bec3b07586a4b085a78cbb97e9bab6f1d3c9ebf299b65dec85213c5eacd44487de86017183120bb7ea3b6c6660c5037615fe1add2a73f800cbeeae22c60438",
                    "03e1a1cfa9eaff604ae237b7af31ffe4c01be22eb96f3da0e62c5850dd4b4386c1",
                    "028d3a2d9f1b1c5c75845944f93bc183ba23aecde53f1978b8aa1b77661be6114f",
                    "028bde91b10013e08949a318018fedbd896534a549a278e220169ee2a36517c7aa",
                    "04c4b0bbb339aa236bff38dbe6a451e111972a7909a126bc424013cba2ec33bc38e98ac269ffe028345c31ac8d0a365f29c8f7e7cfccac72f84e1acd02bc554f35",
                ]),
                expect: fmt(vec![
                    "0234dd69c56c36a41230d573d68adeae0030c9bc0bf26f24d3e1b64c604d293c68",
                    "028bde91b10013e08949a318018fedbd896534a549a278e220169ee2a36517c7aa",
                    "028d3a2d9f1b1c5c75845944f93bc183ba23aecde53f1978b8aa1b77661be6114f",
                    "028e1c947c8c0b8ed021088b8e981491ac7af2b8fabebea1abdb448424c8ed75b7",
                    "02c690d642c1310f3a1ababad94e3930e4023c930ea472e7f37f660fe485263b88",
                    "03004a8a3d242d7957c0b60fb7208d386fa6a0193aabd1f3f095ffd0ac097e447b",
                    "032b8324c93575034047a52e9bca05a46d8347046b91a032eff07d5de8d3f2730b",
                    "038f47dcd43ba6d97fc9ed2e3bba09b175a45fac55f0683e8cf771e8ced4572354",
                    "03e1a1cfa9eaff604ae237b7af31ffe4c01be22eb96f3da0e62c5850dd4b4386c1",
                    "041a181bd0e79974bd7ca552e09fc42ba9c3d5dbb3753741d6f0ab3015dbfd9a22d6b001a32f5f51ac6f2c0f35e73a6a62f59e848fa854d3d21f3f231594eeaa46",
                    "04516cde23e14f2319423b7a4a7ae48b1dadceb5e9c123198d417d10895684c42eb05e210f90ccbc72448803a22312e3f122ff2939956ccef4f7316f836295ddd5",
                    "045d753414fa292ea5b8f56e39cfb6a0287b2546231a5cb05c4b14ab4b463d171f5128148985b23eccb1e2905374873b1f09b9487f47afa6b1f2b0083ac8b4f7e8",
                    // These two pubkeys are mirrored. This helps verify the sort past the x value.
                    "04c4b0bbb339aa236bff38dbe6a451e111972a7909a126bc424013cba2ec33bc3816753d96001fd7cba3ce5372f5c9a0d63708183033538d07b1e532fc43aaacfa",
                    "04c4b0bbb339aa236bff38dbe6a451e111972a7909a126bc424013cba2ec33bc38e98ac269ffe028345c31ac8d0a365f29c8f7e7cfccac72f84e1acd02bc554f35",
                    "04c6bec3b07586a4b085a78cbb97e9bab6f1d3c9ebf299b65dec85213c5eacd44487de86017183120bb7ea3b6c6660c5037615fe1add2a73f800cbeeae22c60438",
                    "04eb0db2d71ccbb0edd8fb35092cbcae2f7fa1f06d4c170804bf52007924b569a8d2d6f6bc8fd2b3caa3253fa1bb674443743bf7fb9f94f9c0b0831a252894cfa8",
                ]),
            },
        ];
        for mut vector in vectors {
            vector.input.sort_by_cached_key(|k| PublicKey::to_sort_key(*k));
            assert_eq!(vector.input, vector.expect);
        }
    }

    #[test]
    #[cfg(feature = "rand-std")]
    fn public_key_constructors() {
        use secp256k1::rand;

        let secp = Secp256k1::new();
        let kp = Keypair::new(&secp, &mut rand::thread_rng());

        let _ = PublicKey::new(kp);
        let _ = PublicKey::new_uncompressed(kp);
    }

    #[test]
    fn public_key_from_str_wrong_length() {
        // Sanity checks, we accept string length 130 digits.
        let s = "042e58afe51f9ed8ad3cc7897f634d881fdbe49a81564629ded8156bebd2ffd1af191923a2964c177f5b5923ae500fca49e99492d534aa3759d6b25a8bc971b133";
        assert_eq!(s.len(), 130);
        assert!(PublicKey::from_str(s).is_ok());
        // And 66 digits.
        let s = "032e58afe51f9ed8ad3cc7897f634d881fdbe49a81564629ded8156bebd2ffd1af";
        assert_eq!(s.len(), 66);
        assert!(PublicKey::from_str(s).is_ok());

        let s = "aoeusthb";
        assert_eq!(s.len(), 8);
        let res = PublicKey::from_str(s);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err(), Error::InvalidHexLength(8));
    }
}
