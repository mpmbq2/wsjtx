program ft8_stage3_dump
  use packjt77
  include 'ft8_params.f90'

  integer, parameter :: NP2 = 2812
  integer, parameter :: MAXCAND = 1000

  character(len=512) :: in_path, out_dir, arg
  character(len=12) :: mycall12, hiscall12
  integer :: nfa_in, nfb_in, nfqso_in, max_samples, ncontest_in
  integer :: nfa0, nfb0, ncand, ic, ncand_dump, imetric, nsym, ihalf, ks, nt
  integer :: i, i0, ibest, idt, ifr, iloc(1), i1, k, i32, ibmax, ib, it1, it2, it3
  integer :: is1, is2, is3, nsync, ip(1), ntype, nharderror, nbadcrc, did_sub
  integer*2 :: iwave(NMAX)
  real :: dd(NMAX), dd_sub_t(NMAX)
  real :: sbase(NH1), candidate(3, MAXCAND)
  real :: ss(9), s8(0:7, NN), s2(0:511)
  real :: bmeta(174), bmetb(174), bmetc(174), bmetd(174), bmete(174)
  real :: bmeta_raw(174), bmetb_raw(174), bmetc_raw(174), bmetd_raw(174), bmete_raw(174)
  real :: llra(174), llrb(174), llrc(174), llrd(174), llre(174)
  real :: f1, xdt, smax, delf, delfbest, dphi, phi, twopi, dt2, fs2, sync, sync_fine, dmin
  real :: m1, m0, bm, den, cm, temp(3), scalefac
  complex :: cd0(0:3199), cd0_ref(0:3199), ctwk(32), csymb(32), cs(0:7, NN)
  integer :: itone(NN), icos7(0:6), graymap(0:7)
  integer :: apsym(58), aph10(10)
  integer*1 :: apmask(174), cw(174), message91(91), message77(77)
  logical :: one(0:511, 0:8)
  integer :: npatience, nthreads
  common/patience/npatience,nthreads

  data icos7/3,1,4,0,6,5,2/
  data graymap/0,1,3,2,5,6,4,7/

  npatience = 0
  nthreads = 1
  one = .false.
  do i = 0, 511
     do k = 0, 8
        if (iand(i, 2**k) .ne. 0) one(i, k) = .true.
     enddo
  enddo

  if (command_argument_count() .lt. 9) then
     write(0,*) 'usage: ft8_stage3_dump <raw_i16.bin> <out_dir> <nfa> <nfb> <nfqso> <max_samples> <mycall> <hiscall> <ncontest>'
     stop 1
  endif

  call get_command_argument(1, in_path)
  call get_command_argument(2, out_dir)
  call get_command_argument(3, arg); read(arg, *) nfa_in
  call get_command_argument(4, arg); read(arg, *) nfb_in
  call get_command_argument(5, arg); read(arg, *) nfqso_in
  call get_command_argument(6, arg); read(arg, *) max_samples
  call get_command_argument(7, mycall12)
  call get_command_argument(8, hiscall12)
  call get_command_argument(9, arg); read(arg, *) ncontest_in

  if (trim(mycall12) .eq. '-') mycall12 = '            '
  if (trim(hiscall12) .eq. '-') hiscall12 = '            '

  if (max_samples .gt. NMAX) max_samples = NMAX
  if (max_samples .lt. 0) max_samples = 0

  iwave = 0
  open(unit=10, file=trim(in_path), access='stream', form='unformatted', status='old')
  read(10) iwave
  close(10)

  dd = 0.0
  do i = 1, max_samples
     dd(i) = real(iwave(i))
  enddo

  ! Dump ft8apset output for provided (mycall12, hiscall12, ncontest_in)
  mycall13 = mycall12
  dxcall13 = hiscall12
  call ft8apset(mycall12, hiscall12, ncontest_in, apsym, aph10)
  open(unit=11, file=trim(out_dir)//'/apset.bin', access='stream', form='unformatted', status='replace')
  write(11) int(apsym, 4), int(aph10, 4)
  close(11)

  nfa0 = nfa_in
  nfb0 = nfb_in
  call get_spectrum_baseline(dd, nfa0, nfb0, sbase)

  nfa0 = nfa_in
  nfb0 = nfb_in
  candidate = 0.0
  ncand = 0
  call sync8(dd, NMAX, nfa0, nfb0, 1.3, nfqso_in, MAXCAND, candidate, ncand, sbase)
  open(unit=12, file=trim(out_dir)//'/sync8.bin', access='stream', form='unformatted', status='replace')
  write(12) int(ncand, 4)
  if (ncand .gt. 0) then
     write(12) candidate(1:3, 1:ncand)
  endif
  close(12)

  twopi = 8.0 * atan(1.0)
  fs2 = 12000.0 / NDOWN
  dt2 = 1.0 / fs2
  ncand_dump = min(ncand, 10)
  did_sub = 0
  scalefac = 2.83

  do ic = 1, ncand_dump
     f1 = candidate(1, ic)
     xdt = candidate(2, ic)

     call ft8_downsample(dd, (ic .eq. 1), f1, cd0)
     i0 = nint((xdt + 0.5) * fs2)
     smax = 0.0
     ibest = 0
     ctwk = cmplx(1.0, 0.0)
     do idt = i0 - 10, i0 + 10
        call sync8d(cd0, idt, ctwk, 0, sync)
        if (sync .gt. smax) then
           smax = sync
           ibest = idt
        endif
     enddo

     smax = 0.0
     delfbest = 0.0
     do ifr = -5, 5
        delf = ifr * 0.5
        dphi = twopi * delf * dt2
        phi = 0.0
        do i = 1, 32
           ctwk(i) = cmplx(cos(phi), sin(phi))
           phi = mod(phi + dphi, twopi)
        enddo
        call sync8d(cd0, ibest, ctwk, 1, sync)
        if (sync .gt. smax) then
           smax = sync
           delfbest = delf
        endif
     enddo

     f1 = f1 + delfbest
     call ft8_downsample(dd, .false., f1, cd0_ref)
     do idt = -4, 4
        call sync8d(cd0_ref, ibest + idt, ctwk, 0, sync)
        ss(idt + 5) = sync
     enddo
     sync_fine = maxval(ss)
     iloc = maxloc(ss)
     ibest = iloc(1) - 5 + ibest
     xdt = (ibest - 1) * dt2

     do k = 1, NN
        i1 = ibest + (k - 1) * 32
        csymb = cmplx(0.0, 0.0)
        if (i1 .ge. 0 .and. i1 + 31 .le. NP2 - 1) csymb = cd0_ref(i1:i1+31)
        call four2a(csymb, 32, 1, -1, 1)
        cs(0:7, k) = csymb(1:8) / 1.0e3
        s8(0:7, k) = abs(csymb(1:8))
     enddo

     is1 = 0; is2 = 0; is3 = 0
     do k = 1, 7
        ip = maxloc(s8(:, k))
        if (icos7(k - 1) .eq. (ip(1) - 1)) is1 = is1 + 1
        ip = maxloc(s8(:, k + 36))
        if (icos7(k - 1) .eq. (ip(1) - 1)) is2 = is2 + 1
        ip = maxloc(s8(:, k + 72))
        if (icos7(k - 1) .eq. (ip(1) - 1)) is3 = is3 + 1
     enddo
     nsync = is1 + is2 + is3

     write(arg, '(A,"/cand3_",I2.2,".bin")') trim(out_dir), ic
     open(unit=13, file=trim(arg), access='stream', form='unformatted', status='replace')
     write(13) f1, xdt, int(nsync, 4), cs, s8

     do imetric = 1, 2
        bmeta_raw = 0.0; bmetb_raw = 0.0; bmetc_raw = 0.0; bmetd_raw = 0.0; bmete_raw = 0.0
        do nsym = 1, 3
           nt = 2**(3 * nsym)
           do ihalf = 1, 2
              do k = 1, 29, nsym
                 if (ihalf .eq. 1) ks = k + 7
                 if (ihalf .eq. 2) ks = k + 43
                 do i = 0, nt - 1
                    it1 = i / 64
                    it2 = iand(i, 63) / 8
                    it3 = iand(i, 7)
                    if (nsym .eq. 1) then
                       s2(i) = abs(cs(graymap(it3), ks))
                    elseif (nsym .eq. 2) then
                       s2(i) = abs(cs(graymap(it2), ks) + cs(graymap(it3), ks + 1))
                    else
                       s2(i) = abs(cs(graymap(it1), ks) + cs(graymap(it2), ks + 1) + cs(graymap(it3), ks + 2))
                    endif
                 enddo
                 if (imetric .eq. 2) s2(0:nt-1) = s2(0:nt-1)**2
                 i32 = 1 + (k - 1) * 3 + (ihalf - 1) * 87
                 if (nsym .eq. 1) ibmax = 2
                 if (nsym .eq. 2) ibmax = 5
                 if (nsym .eq. 3) ibmax = 8
                 do ib = 0, ibmax
                    m1 = maxval(s2(0:nt-1), one(0:nt-1, ibmax - ib))
                    m0 = maxval(s2(0:nt-1), .not. one(0:nt-1, ibmax - ib))
                    bm = m1 - m0
                    if (i32 + ib .gt. 174) cycle
                    if (nsym .eq. 1) then
                       bmeta_raw(i32 + ib) = bm
                       den = max(m1, m0)
                       if (den .gt. 0.0) then
                          cm = bm / den
                       else
                          cm = 0.0
                       endif
                       bmetd_raw(i32 + ib) = cm
                    elseif (nsym .eq. 2) then
                       bmetb_raw(i32 + ib) = bm
                    else
                       bmetc_raw(i32 + ib) = bm
                    endif
                 enddo
              enddo
           enddo
        enddo

        do i = 1, 174
           temp(1) = bmeta_raw(i)
           temp(2) = bmetb_raw(i)
           temp(3) = bmetc_raw(i)
           ip = maxloc(abs(temp))
           bmete_raw(i) = temp(ip(1))
        enddo

        bmeta = bmeta_raw; bmetb = bmetb_raw; bmetc = bmetc_raw; bmetd = bmetd_raw; bmete = bmete_raw
        call normalizebmet(bmeta, 174)
        call normalizebmet(bmetb, 174)
        call normalizebmet(bmetc, 174)
        call normalizebmet(bmetd, 174)
        call normalizebmet(bmete, 174)
        llra = scalefac * bmeta
        llrb = scalefac * bmetb
        llrc = scalefac * bmetc
        llrd = scalefac * bmetd
        llre = scalefac * bmete

        write(13) bmeta_raw, bmetb_raw, bmetc_raw, bmetd_raw, bmete_raw
        write(13) llra, llrb, llrc, llrd, llre
     enddo
     close(13)

     if (did_sub .eq. 0) then
        apmask = 0
        cw = 0
        message91 = 0
        ntype = 0
        nharderror = -1
        dmin = 0.0
        call decode174_91(llra, 91, 2, 2, apmask, message91, cw, ntype, nharderror, dmin)
        if (nharderror .ge. 0 .and. count(cw .eq. 0) .ne. 174) then
           did_sub = 1
           message77 = message91(1:77)
           call get_ft8_tones_from_77bits(message77, itone)
           dd_sub_t = dd
           call subtractft8(dd_sub_t, itone, f1, xdt, .true.)
           open(unit=14, file=trim(out_dir)//'/sub_t.bin', access='stream', form='unformatted', status='replace')
           write(14) int(ic, 4), f1, xdt, itone, dd_sub_t
           close(14)
        endif
     endif
  enddo

end program ft8_stage3_dump
